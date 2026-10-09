//! The deterministic CI gate `ns run` runs after build (docs/FACTORY.md, "Gate").

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::eval::trial::run_process;
use crate::factory::Factory;
use crate::git::same_sha;

/// Lines of gate output kept for `{feedback}`.
const TAIL_LINES: usize = 40;
/// Bytes of gate output kept for `{feedback}`, at most.
const TAIL_BYTES: usize = 3000;
/// Bytes read from the end of the log to build the tail.
const WINDOW_BYTES: u64 = 16 * 1024;

/// `[phases.build] gate`, else the `ci-local` row of `<repo>/docs/agents/stack.md`, else none.
pub fn command(fac: &Factory, repo_root: &Path) -> Option<String> {
    fac.phase("build").gate.or_else(|| {
        fs::read_to_string(repo_root.join("docs/agents/stack.md"))
            .ok()
            .and_then(|t| ci_local(&t))
    })
}

/// The command in a table row `| ci-local | `<command>` | ... |`: the first cell is `ci-local`
/// (backticks optional), the command is the backticked span that opens the second cell (it may contain `|`).
pub fn ci_local(stack_md: &str) -> Option<String> {
    stack_md.lines().find_map(|line| {
        let (first, rest) = line.trim().strip_prefix('|')?.split_once('|')?;
        if first.trim().trim_matches('`') != "ci-local" {
            return None;
        }
        let cmd = rest
            .trim_start()
            .strip_prefix('`')?
            .split('`')
            .next()?
            .trim();
        (!cmd.is_empty()).then(|| cmd.to_string())
    })
}

pub struct GateRun {
    pub exit: Option<i32>,
    pub timed_out: bool,
    pub wall_s: f64,
    /// The last lines of stdout and stderr together.
    pub tail: String,
}

impl GateRun {
    pub fn green(&self) -> bool {
        !self.timed_out && self.exit == Some(0)
    }
}

/// Run `cmd` with `sh -c` in `cwd`, output to `log`, killed at `timeout`.
pub fn run(cmd: &str, cwd: &Path, timeout: Duration, log: &Path) -> Result<GateRun> {
    if let Some(d) = log.parent() {
        fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    let out = File::create(log).with_context(|| format!("cannot create {}", log.display()))?;
    let mut c = Command::new("sh");
    crate::git::scrub(&mut c);
    c.arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdout(out.try_clone()?)
        .stderr(out);
    let (status, timed_out, wall_s) =
        run_process(c, None, timeout).with_context(|| format!("cannot run gate {cmd:?}"))?;
    Ok(GateRun {
        exit: status.and_then(|s| s.code()),
        timed_out,
        wall_s,
        tail: tail(&read_window(log)),
    })
}

/// The end of the file, from a line start unless the whole file fits.
fn read_window(path: &Path) -> Vec<u8> {
    let read = || -> std::io::Result<Vec<u8>> {
        let mut f = File::open(path)?;
        let len = f.metadata()?.len();
        let start = len.saturating_sub(WINDOW_BYTES);
        f.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        if start > 0 {
            if let Some(i) = buf.iter().position(|b| *b == b'\n') {
                buf.drain(..=i);
            }
        }
        Ok(buf)
    };
    read().unwrap_or_default()
}

/// The `{feedback}` for a red gate.
pub fn feedback(cmd: &str, r: &GateRun, timeout: Duration, head: &str) -> String {
    let how = match (r.timed_out, r.exit) {
        (true, _) if timeout.as_secs() >= 60 && timeout.as_secs().is_multiple_of(60) => {
            format!("timed out after {} min", timeout.as_secs() / 60)
        }
        (true, _) => format!("timed out after {:.1} s", timeout.as_secs_f64()),
        (false, Some(c)) => format!("exited {c}"),
        (false, None) => "was killed by a signal".into(),
    };
    format!(
        "The CI gate `{cmd}` {how} at {head}. Make it pass. The last lines of its output:\n\n{}\n",
        r.tail
    )
}

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let kept = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    let start = kept.len().saturating_sub(TAIL_BYTES);
    let start = (start..kept.len())
        .find(|i| kept.is_char_boundary(*i))
        .unwrap_or(0);
    kept[start..].to_string()
}

/// The build phase's timeout; `NS_GATE_TIMEOUT_MS` overrides it.
pub fn timeout(build_minutes: u64) -> Duration {
    std::env::var("NS_GATE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_secs(build_minutes * 60))
}

enum Verdict {
    Green { sha: String },
    Red { sha: String, feedback: String },
}

/// What the gate last said about this unit's HEAD.
#[derive(Default)]
pub struct Gate {
    verdict: Option<Verdict>,
}

pub struct Job<'a> {
    pub cmd: &'a str,
    pub worktree: &'a Path,
    pub log_dir: PathBuf,
    pub unit: &'a str,
    pub timeout: Duration,
    pub head: String,
    pub phase: &'a str,
    pub attempt: u32,
}

/// Why the gate may need to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    BuildPassed,
    ReviewMovedHead,
    BuildWroteNothing,
}

/// Feedback to send back to build, and why.
pub struct Red {
    pub feedback: String,
    pub reason: String,
}

impl Gate {
    /// Run the gate when `trigger` calls for it, unless it already went green at HEAD.
    pub fn after(
        &mut self,
        trigger: Trigger,
        job: &Job<'_>,
        log: &dyn Fn(Value),
    ) -> Result<Option<Red>> {
        match trigger {
            Trigger::BuildPassed | Trigger::ReviewMovedHead => self.run_at_head(job, log),
            Trigger::BuildWroteNothing => self.after_silent_build(job, log),
        }
    }

    fn after_silent_build(&mut self, job: &Job<'_>, log: &dyn Fn(Value)) -> Result<Option<Red>> {
        match &self.verdict {
            Some(Verdict::Red { sha, feedback }) if same_sha(sha, &job.head) => Ok(Some(Red {
                feedback: feedback.clone(),
                reason: "the CI gate is still red".into(),
            })),
            Some(Verdict::Red { .. }) => self.run_at_head(job, log),
            _ => Ok(None),
        }
    }

    fn run_at_head(&mut self, job: &Job<'_>, log: &dyn Fn(Value)) -> Result<Option<Red>> {
        if matches!(&self.verdict, Some(Verdict::Green { sha }) if same_sha(sha, &job.head)) {
            return Ok(None);
        }
        let log_path = job
            .log_dir
            .join(format!("gate-{}-{}.log", job.phase, job.attempt));
        eprintln!("ns run: {} gate after {}: {}", job.unit, job.phase, job.cmd);
        let r = run(job.cmd, job.worktree, job.timeout, &log_path)?;
        log(json!({
            "event": "gate",
            "phase": job.phase,
            "attempt": job.attempt,
            "command": job.cmd,
            "sha": job.head,
            "exit": r.exit,
            "timed_out": r.timed_out,
            "wall_s": (r.wall_s * 10.0).round() / 10.0,
            "green": r.green(),
            "log": log_path.to_string_lossy(),
        }));
        if r.green() {
            self.verdict = Some(Verdict::Green {
                sha: job.head.clone(),
            });
            return Ok(None);
        }
        let feedback = feedback(job.cmd, &r, job.timeout, &job.head);
        self.verdict = Some(Verdict::Red {
            sha: job.head.clone(),
            feedback: feedback.clone(),
        });
        Ok(Some(Red {
            feedback,
            reason: format!("the CI gate failed after {}", job.phase),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::parse;

    const STACK: &str = "| Task | Command |\n|---|---|\n| build | `cargo build` |\n| ci-local | `scripts/ci-local.sh --fast` | 30 s |\n";

    #[test]
    fn ci_local_reads_the_table_row() {
        assert_eq!(
            ci_local(STACK).as_deref(),
            Some("scripts/ci-local.sh --fast")
        );
        assert_eq!(
            ci_local("| `ci-local` | `make ci` (all of CI) |").as_deref(),
            Some("make ci")
        );
        assert_eq!(ci_local("| build | `cargo build` |"), None);
        assert_eq!(ci_local("ci-local: `make ci`"), None);
        assert_eq!(ci_local("| ci-local | none |"), None);
        assert_eq!(ci_local("| ci-local | `` |"), None);
        assert_eq!(ci_local("| ci-local | n/a | see `docs` |"), None);
    }

    #[test]
    fn ci_local_keeps_pipes_inside_the_command() {
        assert_eq!(
            ci_local("| ci-local | `make ci | tee x` | 1 s |").as_deref(),
            Some("make ci | tee x")
        );
        assert_eq!(
            ci_local("| ci-local | `a || b` |").as_deref(),
            Some("a || b")
        );
    }

    #[test]
    fn the_configured_gate_wins_over_stack_md_and_neither_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let none = parse("").unwrap();
        assert_eq!(command(&none, tmp.path()), None);
        fs::create_dir_all(tmp.path().join("docs/agents")).unwrap();
        fs::write(tmp.path().join("docs/agents/stack.md"), STACK).unwrap();
        assert_eq!(
            command(&none, tmp.path()).as_deref(),
            Some("scripts/ci-local.sh --fast")
        );
        let set = parse("[phases.build]\ngate = \"make ci\"\n").unwrap();
        assert_eq!(command(&set, tmp.path()).as_deref(), Some("make ci"));
    }

    #[test]
    fn this_repos_gate_is_ci_local_sh() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let cmd = command(&parse("").unwrap(), repo).unwrap();
        assert_eq!(cmd, "scripts/ci-local.sh");
        assert!(repo.join(&cmd).is_file());
    }

    fn gate(cmd: &str, timeout: Duration) -> (GateRun, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let r = run(cmd, tmp.path(), timeout, &tmp.path().join("logs/gate.log")).unwrap();
        (r, tmp)
    }

    #[test]
    fn a_zero_exit_is_green_and_runs_in_cwd() {
        let (r, tmp) = gate("touch here && echo ok", Duration::from_secs(30));
        assert!(r.green());
        assert_eq!(r.exit, Some(0));
        assert_eq!(r.tail, "ok");
        assert!(tmp.path().join("here").exists());
    }

    #[test]
    fn a_non_zero_exit_is_red_with_stdout_and_stderr_in_the_tail() {
        let (r, _tmp) = gate("echo out; echo err >&2; exit 3", Duration::from_secs(30));
        assert!(!r.green());
        assert_eq!(r.exit, Some(3));
        assert_eq!(r.tail, "out\nerr");
    }

    #[test]
    fn a_timeout_is_red() {
        let (r, _tmp) = gate("sleep 30", Duration::from_millis(200));
        assert!(r.timed_out);
        assert!(!r.green());
        assert!(r.wall_s < 10.0, "{}", r.wall_s);
    }

    #[test]
    fn the_tail_keeps_the_last_lines_within_the_byte_cap() {
        let many: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let t = tail(many.as_bytes());
        assert_eq!(t.lines().count(), TAIL_LINES);
        assert!(t.ends_with("line 100"));
        assert!(t.starts_with("line 61"));
        let long = format!("x{}", "é".repeat(4000));
        let t = tail(long.as_bytes());
        assert!(t.len() <= TAIL_BYTES);
        assert!(t.chars().all(|c| c == 'é'));
        assert!(t.len() > TAIL_BYTES - 2);
    }

    #[test]
    fn a_signal_kill_is_red_with_no_exit_code() {
        let (r, _tmp) = gate("kill -9 $$", Duration::from_secs(30));
        assert_eq!(r.exit, None);
        assert!(!r.timed_out);
        assert!(!r.green());
    }

    #[test]
    fn the_tail_reads_only_the_end_of_a_huge_log() {
        let (r, _tmp) = gate("seq 1 100000", Duration::from_secs(30));
        assert!(r.green());
        assert_eq!(r.tail.lines().count(), TAIL_LINES);
        assert!(r.tail.starts_with("99961"), "{}", r.tail);
        assert!(r.tail.ends_with("100000"));
    }

    #[test]
    fn the_window_drops_a_leading_partial_line() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("big.log");
        let text: String = (0..5000).map(|i| format!("line {i:05}\n")).collect();
        fs::write(&log, &text).unwrap();
        let w = String::from_utf8(read_window(&log)).unwrap();
        assert!((w.len() as u64) <= WINDOW_BYTES);
        assert!(w.starts_with("line "), "{w:?}");
        assert!(w.ends_with("line 04999\n"));
        fs::write(&log, "small\n").unwrap();
        assert_eq!(read_window(&log), b"small\n");
        assert!(read_window(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn a_long_final_line_without_a_newline_still_leaves_a_tail() {
        let (r, _tmp) = gate(
            "head -c 20000 /dev/zero | tr '\\0' x",
            Duration::from_secs(30),
        );
        assert!(r.green());
        assert!(!r.tail.is_empty());
        assert!(r.tail.chars().all(|c| c == 'x'));
    }

    fn red(timed_out: bool, exit: Option<i32>) -> GateRun {
        GateRun {
            exit,
            timed_out,
            wall_s: 0.0,
            tail: "last line".into(),
        }
    }

    #[test]
    fn feedback_says_how_the_gate_failed() {
        let t = Duration::from_secs(90 * 60);
        let f = feedback("make ci", &red(true, None), t, "abc123");
        assert!(
            f.contains("`make ci` timed out after 90 min at abc123"),
            "{f}"
        );
        assert!(f.ends_with("last line\n"));
        let f = feedback(
            "make ci",
            &red(true, None),
            Duration::from_millis(1500),
            "a",
        );
        assert!(f.contains("timed out after 1.5 s"), "{f}");
        let f = feedback("make ci", &red(false, Some(3)), t, "a");
        assert!(f.contains("exited 3"), "{f}");
        let f = feedback("make ci", &red(false, None), t, "a");
        assert!(f.contains("was killed by a signal"), "{f}");
    }
}
