//! The deterministic CI gate `ns run` runs after build, after any phase that moves HEAD, and on
//! the PR head before it merges (docs/FACTORY.md, "Gate").

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::eval::trial::run_process;
use crate::factory::Factory;
use crate::git::same_sha;
use crate::memcap::{self, Cap, Cut};
use crate::ratchet;
use crate::shell::shell_quote;
use crate::stack::rows;

/// Lines of gate output kept for `{feedback}`.
const TAIL_LINES: usize = 40;
/// Bytes of gate output kept for `{feedback}`, at most.
const TAIL_BYTES: usize = 3000;
/// Bytes read from the end of the log to build the tail.
const WINDOW_BYTES: u64 = 16 * 1024;

/// `[phases.build] gate`, else the `ci-local` row of `<repo>/docs/agents/stack.md`, then
/// `<ns> rules check` when that file has a `rules` row, then `<ns> ratchet check --human` when
/// the factory defines a ratchet; none when there is none of them.
pub fn command(fac: &Factory, repo_root: &Path, ns: &Path) -> Option<String> {
    let stack = fs::read_to_string(repo_root.join("docs/agents/stack.md")).unwrap_or_default();
    let base = fac.phase("build").gate.or_else(|| ci_local(&stack));
    let ns = shell_quote(&ns.to_string_lossy());
    let rules = (!crate::rules::commands(&stack).is_empty()).then(|| format!("{ns} rules check"));
    let ratchet = (!fac.ratchets.is_empty()).then(|| format!("{ns} ratchet check --human"));
    let checks: Vec<String> = [rules, ratchet].into_iter().flatten().collect();
    match base {
        Some(b) if checks.is_empty() => Some(b),
        Some(b) => Some(format!("({b}) && {}", checks.join(" && "))),
        None if checks.is_empty() => None,
        None => Some(checks.join(" && ")),
    }
}

/// The command in a table row `| ci-local | `<command>` | ... |`: the first cell is `ci-local`
/// (backticks optional), the command is the backticked span that opens the second cell (it may contain `|`).
pub fn ci_local(stack_md: &str) -> Option<String> {
    rows(stack_md, "ci-local").find_map(|rest| {
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
    /// The memory cap or a signal `ns` didn't send, when either ended it.
    pub cut: Option<Cut>,
    pub wall_s: f64,
    /// The last lines of stdout and stderr together.
    pub tail: String,
    /// The ratchets its `ns ratchet check` failed, one clause each.
    pub ratchets: Option<String>,
}

impl GateRun {
    pub fn green(&self) -> bool {
        !self.timed_out && self.cut.is_none() && self.exit == Some(0)
    }

    /// How it failed at the cap or by a signal, which a plain exit code would hide.
    pub fn killed(&self) -> Option<String> {
        self.cut.map(Cut::reason)
    }
}

/// Run `cmd` with `sh -c` in `cwd` under the memory `cap`, output to `log`, killed at `timeout`.
/// A `ns ratchet check` in `cmd` reads the ratchets of `ratchets.factory`, runs each twice only on
/// its first check of the night, and writes its report next to `log`.
pub fn run(
    cmd: &str,
    cwd: &Path,
    timeout: Duration,
    log: &Path,
    cap: Option<&Cap>,
    ratchets: &Ratchets<'_>,
) -> Result<GateRun> {
    if let Some(d) = log.parent() {
        fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    let out = File::create(log).with_context(|| format!("cannot create {}", log.display()))?;
    let report = log.with_extension("ratchets.json");
    match fs::remove_file(&report) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("cannot remove {}", report.display()))
        }
        _ => {}
    }
    let (mut c, watch) = memcap::command(cap, "sh");
    crate::git::scrub(&mut c);
    c.env(ratchet::FACTORY_VAR, std::path::absolute(ratchets.factory)?)
        .env(ratchet::REPORT_VAR, &report);
    match ratchets.night {
        Some(n) => c.env(ratchet::STEADY_VAR, n.join("ratchets-steady.json")),
        None => c.env_remove(ratchet::STEADY_VAR),
    };
    c.arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdout(out.try_clone()?)
        .stderr(out);
    let (status, timed_out, wall_s) =
        run_process(c, None, timeout).with_context(|| format!("cannot run gate {cmd:?}"))?;
    let output = output_tail(&read_window(log));
    let cut = watch.cut_short(status, timed_out, || output.clone());
    Ok(GateRun {
        exit: status.and_then(|s| s.code()),
        timed_out,
        cut,
        wall_s,
        tail: output,
        ratchets: ratchet::failures(&report),
    })
}

/// Where the gate's `ns ratchet check` finds its ratchets and tonight's steady ones.
pub struct Ratchets<'a> {
    pub factory: &'a Path,
    /// The night directory under `ns watch`; without one every check runs each ratchet twice.
    pub night: Option<&'a Path>,
}

/// The end of the file, from a line start unless the whole file fits.
pub fn read_window(path: &Path) -> Vec<u8> {
    tail(path, WINDOW_BYTES).unwrap_or_default()
}

/// The last `n` bytes of `path`, from a line start unless the whole file fits or they hold no
/// line break.
pub fn tail(path: &Path, n: u64) -> std::io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(n);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if start > 0 {
        if let Some(i) = buf.iter().position(|b| *b == b'\n') {
            buf.drain(..=i);
        }
    }
    Ok(buf)
}

/// "timed out after 90 min", or in seconds when `timeout` is not whole minutes.
pub fn timed_out_after(timeout: Duration) -> String {
    let secs = timeout.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) && timeout.subsec_nanos() == 0 {
        format!("timed out after {} min", secs / 60)
    } else {
        format!("timed out after {:.1} s", timeout.as_secs_f64())
    }
}

/// The `{feedback}` for a red gate.
pub fn feedback(cmd: &str, r: &GateRun, timeout: Duration, head: &str) -> String {
    let how = match (r.timed_out, r.killed(), r.exit) {
        (true, _, _) => timed_out_after(timeout),
        (false, Some(k), _) => k,
        (false, None, Some(c)) => format!("exited {c}"),
        (false, None, None) => "was killed by a signal".into(),
    };
    let ratchets = r.ratchets.as_ref().map_or(String::new(), |f| {
        format!(" It failed on {f}. Get each value back within its limit; never edit ratchets.json or a [ratchets.*] table, since loosening a ratchet is a human decision.")
    });
    format!(
        "The CI gate `{cmd}` {how} at {head}.{ratchets} Make it pass. The last lines of its output:\n\n{}\n",
        r.tail
    )
}

/// The last `count` of `lines`, joined.
fn last_lines<'a>(lines: impl Iterator<Item = &'a str>, count: usize) -> String {
    let all: Vec<&str> = lines.collect();
    all[all.len().saturating_sub(count)..].join("\n")
}

/// `bytes` as text without trailing whitespace.
fn trimmed_lines(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end().to_string()
}

/// The last `TAIL_LINES` lines of a gate's output, cut to `TAIL_BYTES` on a char boundary.
pub fn output_tail(bytes: &[u8]) -> String {
    let text = trimmed_lines(bytes);
    let kept = last_lines(text.lines(), TAIL_LINES);
    let start = kept.len().saturating_sub(TAIL_BYTES);
    let start = (start..kept.len())
        .find(|i| kept.is_char_boundary(*i))
        .unwrap_or(0);
    kept[start..].to_string()
}

/// The last `count` non-blank lines of a command's stderr.
pub fn stderr_tail(bytes: &[u8], count: usize) -> String {
    let text = trimmed_lines(bytes);
    last_lines(text.lines().filter(|l| !l.trim().is_empty()), count)
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
    Green {
        sha: String,
    },
    Red {
        sha: String,
        feedback: String,
        killed: Option<String>,
        ratchets: Option<String>,
    },
}

/// What the gate last said about this unit's HEAD, and about the merge of a PR head into a base.
#[derive(Default)]
pub struct Gate {
    verdict: Option<Verdict>,
    merged: Option<MergeVerdict>,
}

/// The gate's verdict on the merge of PR head `head` into base `base`: `None` is green.
struct MergeVerdict {
    head: String,
    base: String,
    red: Option<Red>,
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
    pub cap: Option<&'a Cap>,
    pub ratchets: Ratchets<'a>,
}

/// Why the gate may need to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    BuildPassed,
    /// A phase other than build moved HEAD.
    PhaseMovedHead,
    BuildWroteNothing,
    /// The merge step is about to merge the PR head.
    BeforeMerge,
}

/// Feedback to send back to build, and why.
#[derive(Clone)]
pub struct Red {
    pub feedback: String,
    pub reason: String,
    /// "the CI gate exceeded the 64 MB memory limit" or "the CI gate was killed by SIGKILL",
    /// when it failed that way.
    pub killed: Option<String>,
    /// The ratchets the gate failed, naming each one's values.
    pub ratchets: Option<String>,
}

impl Red {
    /// Why the unit is stuck if build runs out of attempts with this gate still red.
    pub fn stuck_reason(&self) -> Option<String> {
        self.killed.clone().or_else(|| {
            self.ratchets
                .as_ref()
                .map(|r| format!("the CI gate failed on {r}"))
        })
    }
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
            Trigger::BuildPassed | Trigger::PhaseMovedHead | Trigger::BeforeMerge => {
                self.run_at_head(job, log)
            }
            Trigger::BuildWroteNothing => self.after_silent_build(job, log),
        }
    }

    /// The feedback of a red verdict not yet followed by a green one.
    pub fn red_feedback(&self) -> Option<&str> {
        match &self.verdict {
            Some(Verdict::Red { feedback, .. }) => Some(feedback),
            _ => None,
        }
    }

    fn after_silent_build(&mut self, job: &Job<'_>, log: &dyn Fn(Value)) -> Result<Option<Red>> {
        match &self.verdict {
            Some(Verdict::Red {
                sha,
                feedback,
                killed,
                ratchets,
            }) if same_sha(sha, &job.head) => Ok(Some(Red {
                feedback: feedback.clone(),
                reason: "the CI gate is still red".into(),
                killed: killed.clone(),
                ratchets: ratchets.clone(),
            })),
            Some(Verdict::Red { .. }) => self.run_at_head(job, log),
            _ => Ok(None),
        }
    }

    fn run_at_head(&mut self, job: &Job<'_>, log: &dyn Fn(Value)) -> Result<Option<Red>> {
        if matches!(&self.verdict, Some(Verdict::Green { sha }) if same_sha(sha, &job.head)) {
            return Ok(None);
        }
        eprintln!("ns run: {} gate after {}: {}", job.unit, job.phase, job.cmd);
        let r = run_logged(job, json!({}), log)?;
        if r.green() {
            self.verdict = Some(Verdict::Green {
                sha: job.head.clone(),
            });
            return Ok(None);
        }
        let feedback = feedback(job.cmd, &r, job.timeout, &job.head);
        let killed = r.killed().map(|k| format!("the CI gate {k}"));
        self.verdict = Some(Verdict::Red {
            sha: job.head.clone(),
            feedback: feedback.clone(),
            killed: killed.clone(),
            ratchets: r.ratchets.clone(),
        });
        let reason = match (&killed, &r.ratchets) {
            (Some(k), _) => format!("{k} after {}", job.phase),
            (None, Some(f)) => format!("the CI gate failed on {f} after {}", job.phase),
            (None, None) => format!("the CI gate failed after {}", job.phase),
        };
        Ok(Some(Red {
            feedback,
            reason,
            killed,
            ratchets: r.ratchets,
        }))
    }

    /// The verdict of the last gate on a merge result, when it was the merge of `head` into
    /// `base`: `Some(None)` is green. Reused until either side moves.
    pub fn merge_verdict(&self, head: &str, base: &str) -> Option<Option<Red>> {
        self.merged
            .as_ref()
            .filter(|v| same_sha(&v.head, head) && same_sha(&v.base, base))
            .map(|v| v.red.clone())
    }

    /// Run the gate in `job.worktree`, which holds the merge of PR head `job.head` into
    /// `default` at `base`, and keep its verdict for [`Gate::merge_verdict`], except a red that a
    /// timeout, the memory cap or a signal caused: that says nothing about the code, so the next
    /// merge step runs the gate again.
    pub fn on_merge_result(
        &mut self,
        job: &Job<'_>,
        default: &str,
        base: &str,
        log: &dyn Fn(Value),
    ) -> Result<Option<Red>> {
        eprintln!(
            "ns run: {} gate on the merge of {} into {default} at {base}: {}",
            job.unit, job.head, job.cmd
        );
        let r = run_logged(job, json!({"merge_result": true, "base": base}), log)?;
        let red = (!r.green()).then(|| {
            let killed = r.killed().map(|k| format!("the CI gate {k}"));
            Red {
                feedback: format!(
                    "{}\nThe gate ran on the merge of the PR head {} into {default} at {base}, not on the branch alone, so the failure may come from combining it with {default}. Merge or rebase onto {default} and make the gate pass there.\n",
                    feedback(job.cmd, &r, job.timeout, &job.head),
                    job.head,
                ),
                reason: match &killed {
                    Some(k) => format!("{k} on the merge with {default}"),
                    None => format!("the CI gate failed on the merge with {default}"),
                },
                killed,
                ratchets: r.ratchets.clone(),
            }
        });
        let environmental = r.timed_out || r.cut.is_some();
        self.merged = (!environmental).then(|| MergeVerdict {
            head: job.head.clone(),
            base: base.to_string(),
            red: red.clone(),
        });
        Ok(red)
    }
}

/// Run `job`'s gate and log a `gate` event, with the fields of `extra` added.
fn run_logged(job: &Job<'_>, mut extra: Value, log: &dyn Fn(Value)) -> Result<GateRun> {
    let log_path = job
        .log_dir
        .join(format!("gate-{}-{}.log", job.phase, job.attempt));
    let r = run(
        job.cmd,
        job.worktree,
        job.timeout,
        &log_path,
        job.cap,
        &job.ratchets,
    )?;
    let mut ev = json!({
        "event": "gate",
        "phase": job.phase,
        "attempt": job.attempt,
        "command": job.cmd,
        "sha": job.head,
        "exit": r.exit,
        "timed_out": r.timed_out,
        "signal": match r.cut {
            Some(Cut::Signal(sig)) => Some(crate::stop::name(sig)),
            _ => None,
        },
        "memory_exceeded": matches!(r.cut, Some(Cut::Exceeded(_))),
        "wall_s": (r.wall_s * 10.0).round() / 10.0,
        "green": r.green(),
        "log": log_path.to_string_lossy(),
    });
    if let (Some(o), Some(x)) = (ev.as_object_mut(), extra.as_object_mut()) {
        o.append(x);
    }
    log(ev);
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::parse;

    const STACK: &str = "| Task | Command |\n|---|---|\n| build | `cargo build` |\n| ci-local | `scripts/ci-local.sh` | 30 s |\n";

    #[test]
    fn ci_local_reads_the_table_row() {
        assert_eq!(ci_local(STACK).as_deref(), Some("scripts/ci-local.sh"));
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
        assert_eq!(command(&none, tmp.path(), Path::new(NS_EXE)), None);
        fs::create_dir_all(tmp.path().join("docs/agents")).unwrap();
        fs::write(tmp.path().join("docs/agents/stack.md"), STACK).unwrap();
        assert_eq!(
            command(&none, tmp.path(), Path::new(NS_EXE)).as_deref(),
            Some("scripts/ci-local.sh")
        );
        let set = parse("[phases.build]\ngate = \"make ci\"\n").unwrap();
        assert_eq!(
            command(&set, tmp.path(), Path::new(NS_EXE)).as_deref(),
            Some("make ci")
        );
    }

    const NS_EXE: &str = "/opt/ns bin/ns";

    #[test]
    fn a_rules_row_adds_ns_rules_check_after_the_gate_or_alone() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs/agents")).unwrap();
        let stack = tmp.path().join("docs/agents/stack.md");
        fs::write(&stack, format!("{STACK}| rules | `ruff check .` |\n")).unwrap();
        let none = parse("").unwrap();
        assert_eq!(
            command(&none, tmp.path(), Path::new(NS_EXE)).as_deref(),
            Some("(scripts/ci-local.sh) && '/opt/ns bin/ns' rules check")
        );
        let set = parse("[phases.build]\ngate = \"make ci\"\n").unwrap();
        assert_eq!(
            command(&set, tmp.path(), Path::new(NS_EXE)).as_deref(),
            Some("(make ci) && '/opt/ns bin/ns' rules check")
        );
        fs::write(&stack, "| rules | `ruff check .` |\n").unwrap();
        assert_eq!(
            command(&none, tmp.path(), Path::new(NS_EXE)).as_deref(),
            Some("'/opt/ns bin/ns' rules check")
        );
        fs::write(&stack, "| rules | none yet |\n").unwrap();
        assert_eq!(command(&none, tmp.path(), Path::new(NS_EXE)), None);
    }

    #[test]
    fn a_ratchet_adds_ns_ratchet_check_last_or_alone() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs/agents")).unwrap();
        let stack = tmp.path().join("docs/agents/stack.md");
        let fac = parse("[ratchets.flash]\ncommand = \"echo 1\"\nbetter = \"lower\"\n").unwrap();
        fs::write(&stack, format!("{STACK}| rules | `ruff check .` |\n")).unwrap();
        assert_eq!(
            command(&fac, tmp.path(), Path::new(NS_EXE)).as_deref(),
            Some("(scripts/ci-local.sh) && '/opt/ns bin/ns' rules check && '/opt/ns bin/ns' ratchet check --human")
        );
        fs::write(&stack, STACK).unwrap();
        assert_eq!(
            command(&fac, tmp.path(), Path::new(NS_EXE)).as_deref(),
            Some("(scripts/ci-local.sh) && '/opt/ns bin/ns' ratchet check --human")
        );
        fs::write(&stack, "").unwrap();
        assert_eq!(
            command(&fac, tmp.path(), Path::new(NS_EXE)).as_deref(),
            Some("'/opt/ns bin/ns' ratchet check --human")
        );
    }

    #[test]
    fn this_repos_gate_is_ci_local_sh_then_the_rule_pack() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let cmd = command(&parse("").unwrap(), repo, Path::new(NS_EXE)).unwrap();
        assert_eq!(cmd, "(scripts/ci-local.sh) && '/opt/ns bin/ns' rules check");
        assert!(repo.join("scripts/ci-local.sh").is_file());
    }

    #[test]
    fn the_script_and_both_hooks_are_executable_and_the_hooks_run_full_ci() {
        use std::os::unix::fs::PermissionsExt;
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for f in [
            "scripts/ci-local.sh",
            ".githooks/pre-commit",
            ".githooks/pre-push",
        ] {
            let mode = std::fs::metadata(repo.join(f))
                .unwrap()
                .permissions()
                .mode();
            assert!(mode & 0o111 != 0, "{f} is not executable");
        }
        let text = std::fs::read_to_string(repo.join(".githooks/pre-commit")).unwrap();
        assert_eq!(text.lines().last(), Some("exec scripts/ci-local.sh"));
        // pre-push runs it once per commit pushed; the cli tests pin how.
        let text = std::fs::read_to_string(repo.join(".githooks/pre-push")).unwrap();
        assert!(text.contains("exec scripts/ci-local.sh </dev/null"));
    }

    fn gate(cmd: &str, timeout: Duration) -> (GateRun, tempfile::TempDir) {
        gate_capped(cmd, timeout, None)
    }

    fn gate_capped(
        cmd: &str,
        timeout: Duration,
        cap: Option<&Cap>,
    ) -> (GateRun, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("logs/gate.log");
        let ratchets = Ratchets {
            factory: tmp.path(),
            night: None,
        };
        let r = run(cmd, tmp.path(), timeout, &log, cap, &ratchets).unwrap();
        (r, tmp)
    }

    const REPORT: &str = r#"{"ratchets": [{"name": "flash", "command": "s", "better": "lower", "tolerance": "0", "recorded": 1, "measured": 2, "runs": [2], "limit": 1, "status": "worse"}, {"name": "cov", "command": "c", "better": "higher", "tolerance": "0", "recorded": 1, "measured": 2, "runs": [2], "limit": 1, "status": "better"}]}"#;

    #[test]
    fn the_gate_tells_ns_ratchet_check_its_factory_night_and_report() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("gate-build-1.log");
        let night = tmp.path().join("night");
        let cmd = format!(
            "echo \"$NS_RATCHET_FACTORY|$NS_RATCHET_STEADY|$NS_RATCHET_REPORT\"; printf '%s' '{REPORT}' > \"$NS_RATCHET_REPORT\"; exit 1"
        );
        let with_night = Ratchets {
            factory: Path::new("fac"),
            night: Some(&night),
        };
        let t = Duration::from_secs(30);
        let r = run(&cmd, tmp.path(), t, &log, None, &with_night).unwrap();
        let report = tmp.path().join("gate-build-1.ratchets.json");
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            r.tail,
            format!(
                "{}|{}|{}",
                cwd.join("fac").display(),
                night.join("ratchets-steady.json").display(),
                report.display()
            )
        );
        assert_eq!(
            r.ratchets.as_deref(),
            Some("ratchet flash is worse: measured 2, recorded 1, tolerance 0, limit 1, lower is better")
        );
        // A report left by an earlier gate is gone before the next one runs.
        let no_night = Ratchets {
            factory: tmp.path(),
            night: None,
        };
        let r = run(
            "echo \"[$NS_RATCHET_STEADY]\"",
            tmp.path(),
            t,
            &log,
            None,
            &no_night,
        )
        .unwrap();
        assert_eq!((r.tail.as_str(), r.ratchets), ("[]", None));
        assert!(!report.exists());
        fs::create_dir(&report).unwrap();
        fs::write(report.join("x"), "").unwrap();
        let e = run("true", tmp.path(), t, &log, None, &no_night)
            .err()
            .unwrap();
        assert!(format!("{e:#}").contains("cannot remove"), "{e:#}");
    }

    #[test]
    fn a_ratchet_failure_is_in_the_feedback_reason_and_stuck_reason() {
        let mut g = red(false, Some(1));
        g.ratchets = Some("ratchet flash is worse: measured 2, recorded 1".into());
        let f = feedback("make ci", &g, Duration::from_secs(60), "abc");
        assert!(
            f.contains("at abc. It failed on ratchet flash is worse: measured 2, recorded 1. Get each value back"),
            "{f}"
        );
        assert!(f.contains("never edit ratchets.json"), "{f}");
        let plain = feedback(
            "make ci",
            &red(false, Some(1)),
            Duration::from_secs(60),
            "abc",
        );
        assert!(plain.contains("at abc. Make it pass."), "{plain}");

        let mut red = Red {
            feedback: String::new(),
            reason: String::new(),
            killed: None,
            ratchets: Some("ratchet flash is worse".into()),
        };
        assert_eq!(
            red.stuck_reason().as_deref(),
            Some("the CI gate failed on ratchet flash is worse")
        );
        red.killed = Some("the CI gate was killed by SIGKILL".into());
        assert_eq!(
            red.stuck_reason().as_deref(),
            Some("the CI gate was killed by SIGKILL")
        );
        red.ratchets = None;
        red.killed = None;
        assert_eq!(red.stuck_reason(), None);
    }

    const HOG: &str = "python3 -c \"b = b'x' * (256 << 20)\"";

    #[test]
    fn a_gate_past_the_memory_cap_is_red_and_says_so() {
        let cap = memcap::detect(64).unwrap();
        let (r, _tmp) = gate_capped(HOG, Duration::from_secs(60), Some(&cap));
        assert!(!r.green());
        assert_eq!(r.cut, Some(Cut::Exceeded(64)));
        let f = feedback(HOG, &r, Duration::from_secs(60), "abc123");
        assert!(
            f.contains("exceeded the 64 MB memory limit at abc123"),
            "{f}"
        );
        let (r, _tmp) = gate_capped("echo fits", Duration::from_secs(60), Some(&cap));
        assert!(r.green(), "{}", r.tail);
        assert_eq!(r.cut, None);
    }

    #[test]
    fn a_gate_past_the_rlimit_fallback_is_red_and_says_so() {
        let cap = Cap {
            mb: 64,
            via: memcap::Via::Rlimit,
            fallback: None,
        };
        let (r, _tmp) = gate_capped(HOG, Duration::from_secs(60), Some(&cap));
        assert!(!r.green());
        assert_eq!(r.cut, Some(Cut::Exceeded(64)));
        assert_eq!(
            r.killed().as_deref(),
            Some("exceeded the 64 MB memory limit")
        );
    }

    #[test]
    fn a_capped_gate_that_times_out_is_a_timeout_whatever_it_printed() {
        let cap = Cap {
            mb: 64,
            via: memcap::Via::Rlimit,
            fallback: None,
        };
        let (r, _tmp) = gate_capped(
            "echo MemoryError; exec sleep 30",
            Duration::from_millis(300),
            Some(&cap),
        );
        assert!(r.timed_out);
        assert_eq!(r.cut, None);
    }

    #[test]
    fn an_uncapped_gate_that_fails_with_an_allocation_message_is_an_ordinary_failure() {
        let (r, _tmp) = gate("echo MemoryError; exit 1", Duration::from_secs(30));
        assert_eq!(r.cut, None);
        assert_eq!(r.killed(), None);
        assert_eq!(r.exit, Some(1));
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
    fn the_tail_keeps_blank_lines_between_output_but_not_after_it() {
        let (r, _tmp) = gate("printf 'a\\n\\nb\\n\\n\\n'", Duration::from_secs(30));
        assert_eq!(r.tail, "a\n\nb");
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
        let t = output_tail(many.as_bytes());
        assert_eq!(t.lines().count(), TAIL_LINES);
        assert!(t.ends_with("line 100"));
        assert!(t.starts_with("line 61"));
        let long = format!("x{}", "é".repeat(4000));
        let t = output_tail(long.as_bytes());
        assert!(t.len() <= TAIL_BYTES);
        assert!(t.chars().all(|c| c == 'é'));
        assert!(t.len() > TAIL_BYTES - 2);
    }

    #[test]
    fn a_signal_kill_is_red_with_no_exit_code() {
        let (r, _tmp) = gate("kill -9 $$", Duration::from_secs(30));
        assert_eq!(r.exit, None);
        assert_eq!(r.cut, Some(Cut::Signal(libc::SIGKILL)));
        assert!(!r.timed_out);
        assert!(!r.green());
        assert_eq!(r.killed().as_deref(), Some("was killed by SIGKILL"));
        let f = feedback("x", &r, Duration::from_secs(30), "a");
        assert!(f.contains("`x` was killed by SIGKILL at a"), "{f}");
    }

    #[test]
    fn a_timed_out_gate_is_not_called_killed() {
        let (r, _tmp) = gate("sleep 30", Duration::from_millis(200));
        assert!(r.timed_out);
        assert_eq!(r.killed(), None);
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

    #[test]
    fn timed_out_after_uses_minutes_only_for_whole_minutes() {
        assert_eq!(
            timed_out_after(Duration::from_secs(60)),
            "timed out after 1 min"
        );
        assert_eq!(
            timed_out_after(Duration::from_secs(59)),
            "timed out after 59.0 s"
        );
        assert_eq!(
            timed_out_after(Duration::from_millis(60_500)),
            "timed out after 60.5 s"
        );
    }

    fn red(timed_out: bool, exit: Option<i32>) -> GateRun {
        GateRun {
            exit,
            cut: None,
            timed_out,
            wall_s: 0.0,
            tail: "last line".into(),
            ratchets: None,
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
        let mut r = red(false, Some(1));
        r.cut = Some(Cut::Exceeded(64));
        let f = feedback("make ci", &r, t, "a");
        assert!(
            f.contains("`make ci` exceeded the 64 MB memory limit at a"),
            "{f}"
        );
        r.timed_out = true;
        let f = feedback("make ci", &r, t, "a");
        assert!(f.contains("timed out after 90 min"), "{f}");
    }

    #[test]
    fn a_zero_exit_past_the_cap_is_not_green() {
        let mut r = red(false, Some(0));
        assert!(r.green());
        r.cut = Some(Cut::Exceeded(64));
        assert!(!r.green());
    }
}
