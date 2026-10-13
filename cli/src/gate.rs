//! The deterministic CI gate `ns run` runs after build, after any phase that moves HEAD, and on
//! the PR head before it merges (docs/factory/run.md, "Gate").

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::eval::trial::{run_process, tail};
use crate::factory::{Factory, LocalGate};
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

/// The variable that holds, one per line, the paths a fast gate's `ci-affected` command tests.
pub const FILES_VAR: &str = "NS_GATE_FILES";

/// The gate commands `ns run` runs: `build` after build, `rest` after any other phase and in
/// the merge step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Commands {
    pub build: Option<String>,
    pub rest: Option<String>,
}

impl Commands {
    /// Under `[gate] local = "every-step"` both are [`command`]. Under `"build-only"`, `rest` is
    /// the base command with `--fast`, then the `ci-affected` row of `docs/agents/stack.md`.
    pub fn read(fac: &Factory, repo_root: &Path, ns: &Path) -> Commands {
        let build = command(fac, repo_root, ns);
        let rest = match fac.gate.local {
            LocalGate::EveryStep => build.clone(),
            LocalGate::BuildOnly => {
                let stack =
                    fs::read_to_string(repo_root.join("docs/agents/stack.md")).unwrap_or_default();
                let fast = fac
                    .phase("build")
                    .gate
                    .or_else(|| ci_local(&stack))
                    .map(|b| format!("{b} --fast"));
                let affected = row_command(&stack, "ci-affected");
                let parts: Vec<String> = [fast, affected]
                    .into_iter()
                    .flatten()
                    .map(|c| format!("({c})"))
                    .collect();
                (!parts.is_empty()).then(|| parts.join(" && "))
            }
        };
        Commands { build, rest }
    }

    /// The command `trigger` runs.
    pub fn for_trigger(&self, trigger: Trigger) -> Option<&str> {
        match trigger {
            Trigger::BuildPassed | Trigger::BuildWroteNothing => self.build.as_deref(),
            Trigger::PhaseMovedHead | Trigger::BeforeMerge => self.rest.as_deref(),
        }
    }

    /// Whether `cmd` is the full gate: the one run after build.
    pub fn full(&self, cmd: &str) -> bool {
        self.build.as_deref() == Some(cmd)
    }
}

/// The command in a table row `| ci-local | `<command>` | ... |`: the first cell is `ci-local`
/// (backticks optional), the command is the backticked span that opens the second cell (it may contain `|`).
pub fn ci_local(stack_md: &str) -> Option<String> {
    row_command(stack_md, "ci-local")
}

/// The command in the table row whose first cell is `task`, as [`ci_local`] reads it.
fn row_command(stack_md: &str, task: &str) -> Option<String> {
    rows(stack_md, task).find_map(|rest| {
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
/// its first check of the night, and writes its report next to `log`. `files`, when known, is
/// [`FILES_VAR`].
pub fn run(
    cmd: &str,
    cwd: &Path,
    timeout: Duration,
    log: &Path,
    cap: Option<&Cap>,
    ratchets: &Ratchets<'_>,
    files: Option<&str>,
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
    crate::cpus::apply(&mut c);
    c.env(ratchet::FACTORY_VAR, std::path::absolute(ratchets.factory)?)
        .env(ratchet::REPORT_VAR, &report);
    match ratchets.night {
        Some(n) => c.env(ratchet::STEADY_VAR, n.join("ratchets-steady.json")),
        None => c.env_remove(ratchet::STEADY_VAR),
    };
    match files {
        Some(f) => c.env(FILES_VAR, f),
        None => c.env_remove(FILES_VAR),
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
        /// Whether the full gate went green, which covers the fast one too.
        full: bool,
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
    /// The last HEAD the full gate went green at.
    full_green: Option<String>,
}

/// The gate's verdict on the merge of PR head `head` into base `base`: `None` is green.
struct MergeVerdict {
    head: String,
    base: String,
    red: Option<Red>,
}

pub struct Job<'a> {
    pub cmd: &'a str,
    /// Whether `cmd` is the full gate, logged as `local_full`, else `local_fast`.
    pub full: bool,
    /// The paths changed since the full gate last went green, for a fast gate.
    pub files: Option<String>,
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

    /// The last HEAD the full gate went green at in this invocation.
    pub fn full_green(&self) -> Option<&str> {
        self.full_green.as_deref()
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
        if matches!(&self.verdict, Some(Verdict::Green { sha, full }) if same_sha(sha, &job.head) && (*full || !job.full))
        {
            return Ok(None);
        }
        eprintln!("ns run: {} gate after {}: {}", job.unit, job.phase, job.cmd);
        let r = run_logged(job, json!({}), log)?;
        if r.green() {
            self.verdict = Some(Verdict::Green {
                sha: job.head.clone(),
                full: job.full,
            });
            if job.full {
                self.full_green = Some(job.head.clone());
            }
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
        job.files.as_deref(),
    )?;
    let mut ev = json!({
        "event": "gate",
        "phase": job.phase,
        "attempt": job.attempt,
        "command": job.cmd,
        "where": if job.full { "local_full" } else { "local_fast" },
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
mod tests;
