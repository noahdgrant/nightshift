//! `ns ratchet check` and `ns ratchet update`: measured values (binary size, coverage, warning
//! counts) that may never get worse than the values recorded in `ratchets.json`
//! (docs/factory/run.md, "Ratchets").

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::SfError;
use crate::eval::trial::{run_process, tail};
use crate::factory::{Better, Ratchet};

/// The recorded values, next to `nightshift.toml` in the factory directory.
pub const FILE: &str = "ratchets.json";
/// The factory directory `ns run` hands its gate; `--factory` wins over it.
pub const FACTORY_VAR: &str = "NS_RATCHET_FACTORY";
/// A file naming the ratchets already run twice tonight; unset, every check runs them twice.
pub const STEADY_VAR: &str = "NS_RATCHET_STEADY";
/// Where `ns ratchet check` also writes its JSON, for `ns run` to name a failure.
pub const REPORT_VAR: &str = "NS_RATCHET_REPORT";
const VARS: [&str; 3] = [FACTORY_VAR, STEADY_VAR, REPORT_VAR];
const TIMEOUT: Duration = Duration::from_secs(300);
const MAX_STDOUT_BYTES: u64 = 64 * 1024;
const STDERR_TAIL_BYTES: u64 = 2000;
/// Runs of a ratchet's command on a ratchet's first check: two values show whether it is steady.
const FIRST_CHECK_RUNS: usize = 2;
/// Runs of a ratchet's command once tonight's earlier check found it steady.
const STEADY_RUNS: usize = 1;

#[derive(Serialize, Deserialize, Default)]
struct Recorded {
    version: u32,
    values: BTreeMap<String, f64>,
}

/// The worst value `recorded` allows, tolerance included.
fn limit(r: &Ratchet, recorded: f64) -> f64 {
    match r.better {
        Better::Lower => recorded + r.tolerance.around(recorded),
        Better::Higher => recorded - r.tolerance.around(recorded),
    }
}

fn worse(better: Better, value: f64, than: f64) -> bool {
    match better {
        Better::Lower => value > than,
        Better::Higher => value < than,
    }
}

/// Run `cmd` once and read the one number it printed. The error says how it failed.
fn measure(cmd: &str, root: &Path, timeout: Duration) -> Result<f64, String> {
    let out = tempfile::tempfile().map_err(|e| format!("cannot create a file for output: {e}"))?;
    let err = tempfile::NamedTempFile::new()
        .map_err(|e| format!("cannot create a file for output: {e}"))?;
    let mut c = Command::new("sh");
    crate::git::scrub(&mut c);
    for v in VARS {
        c.env_remove(v);
    }
    let pipe = |f: &File| {
        f.try_clone()
            .map_err(|e| format!("cannot capture output: {e}"))
    };
    c.arg("-c")
        .arg(cmd)
        .current_dir(root)
        .stdout(pipe(&out)?)
        .stderr(pipe(err.as_file())?);
    let (status, timed_out, _) =
        run_process(c, None, timeout).map_err(|e| format!("cannot run: {e}"))?;
    let stderr = tail(err.path(), STDERR_TAIL_BYTES).unwrap_or_default();
    let stderr = String::from_utf8_lossy(&stderr);
    let with_stderr = |how: String| match stderr.trim() {
        "" => how,
        s => format!("{how}; stderr: {s}"),
    };
    if timed_out {
        return Err(with_stderr(format!(
            "timed out after {} s",
            timeout.as_secs_f64()
        )));
    }
    match status.and_then(|s| s.code()) {
        Some(0) => {}
        Some(c) => return Err(with_stderr(format!("exited {c}"))),
        None => return Err(with_stderr("was killed by a signal".into())),
    }
    let mut buf = Vec::new();
    let mut out = out;
    out.seek(SeekFrom::Start(0))
        .and_then(|_| out.take(MAX_STDOUT_BYTES + 1).read_to_end(&mut buf))
        .map_err(|e| format!("cannot read its output: {e}"))?;
    if buf.len() as u64 > MAX_STDOUT_BYTES {
        return Err(format!(
            "printed more than {MAX_STDOUT_BYTES} bytes, not one number"
        ));
    }
    let text = String::from_utf8_lossy(&buf);
    let text = text.trim();
    match text.parse::<f64>() {
        Ok(n) if n.is_finite() => Ok(n),
        _ => Err(with_stderr(format!(
            "printed {:?}, not one number",
            text.chars().take(200).collect::<String>()
        ))),
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
enum Status {
    Ok,
    Better,
    Worse,
    Untracked,
    Broken,
    Flaky,
    Recorded,
    Unchanged,
    Tightened,
    Loosened,
    Kept,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = serde_json::to_value(self).map_err(|_| fmt::Error)?;
        f.write_str(v.as_str().unwrap_or_default())
    }
}

/// What one ratchet measured and how that compares with its recorded value.
#[derive(Serialize, Deserialize, Debug)]
struct Outcome {
    name: String,
    command: String,
    better: Better,
    tolerance: String,
    recorded: Option<f64>,
    measured: Option<f64>,
    /// Both values when the command ran twice.
    runs: Vec<f64>,
    limit: Option<f64>,
    status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Outcome {
    fn failed(&self) -> bool {
        matches!(self.status, Status::Worse | Status::Broken | Status::Flaky)
    }

    fn line(&self) -> String {
        let n = &self.name;
        let tol = &self.tolerance;
        let better = match self.better {
            Better::Lower => "lower",
            Better::Higher => "higher",
        };
        let m = self
            .measured
            .map_or("nothing".to_string(), |m| m.to_string());
        match (self.status, self.recorded) {
            (Status::Broken, _) => format!(
                "ratchet {n} is broken: `{}` {}",
                self.command,
                self.error.as_deref().unwrap_or_default()
            ),
            (Status::Flaky, _) => {
                let runs: Vec<String> = self.runs.iter().map(f64::to_string).collect();
                format!(
                    "ratchet {n} is flaky: two runs of `{}` measured {}, more apart than its tolerance {tol}",
                    self.command,
                    runs.join(" and ")
                )
            }
            (status, None) => format!("ratchet {n} is {status}: measured {m}, nothing recorded"),
            (status, Some(r)) => format!(
                "ratchet {n} is {status}: measured {m}, recorded {r}, tolerance {tol}, limit {}, {better} is better",
                self.limit.unwrap_or(r)
            ),
        }
    }
}

/// Measure `r` `runs` times and compare the worst value with `recorded`.
fn assess(
    name: &str,
    r: &Ratchet,
    recorded: Option<f64>,
    runs: usize,
    root: &Path,
    timeout: Duration,
) -> Outcome {
    eprintln!("ns ratchet: {name}: {}", r.command);
    let mut o = Outcome {
        name: name.to_string(),
        command: r.command.clone(),
        better: r.better,
        tolerance: r.tolerance.to_string(),
        recorded,
        measured: None,
        runs: Vec::new(),
        limit: recorded.map(|v| limit(r, v)),
        status: Status::Broken,
        error: None,
    };
    for _ in 0..runs {
        match measure(&r.command, root, timeout) {
            Ok(v) => o.runs.push(v),
            Err(e) => {
                o.error = Some(e);
                return o;
            }
        }
    }
    let worst = o
        .runs
        .iter()
        .copied()
        .fold(o.runs[0], |w, v| if worse(r.better, v, w) { v } else { w });
    o.measured = Some(worst);
    if let [a, b] = o.runs[..] {
        if (a - b).abs() > r.tolerance.around(recorded.unwrap_or(a)) {
            o.status = Status::Flaky;
            return o;
        }
    }
    o.status = match (recorded, o.limit) {
        (Some(_), Some(l)) if worse(r.better, worst, l) => Status::Worse,
        (Some(rec), _) if worse(r.better, rec, worst) => Status::Better,
        (Some(_), _) => Status::Ok,
        (None, _) => Status::Untracked,
    };
    o
}

struct Ctx {
    root: PathBuf,
    file: PathBuf,
    ratchets: BTreeMap<String, Ratchet>,
    recorded: Recorded,
}

fn ctx(factory: Option<PathBuf>, usage: &str) -> Result<Ctx> {
    let cwd = std::env::current_dir().context("cannot read current directory")?;
    let root = crate::git::run(&cwd, &["rev-parse", "--show-toplevel"]).map_err(|_| {
        SfError::usage(
            format!("{} is not inside a git repository", cwd.display()),
            format!("run it from the repo:\n  cd ~/src/myrepo && {usage}"),
        )
    })?;
    let dir = match factory {
        Some(d) => d,
        None => crate::factory::root(None, &crate::git::Repo::discover(&cwd)?.root),
    };
    let fac = crate::factory::load(&dir)?;
    let file = dir.join(FILE);
    let recorded = match fs::read_to_string(&file) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| {
            SfError::general(format!("{} is not a ratchets file: {e}", file.display())).hint(
                format!(
                    "restore it from git:\n  git checkout -- {}\nor delete it and have a human rerun:\n  ns ratchet update",
                    file.display()
                ),
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Recorded::default(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
    };
    Ok(Ctx {
        root: PathBuf::from(root),
        file,
        ratchets: fac.ratchets,
        recorded,
    })
}

/// `NS_RATCHET_TIMEOUT_MS` when it is a positive number of milliseconds; anything else falls
/// back to the default, as `NS_RULES_TIMEOUT_MS` does.
fn timeout() -> Duration {
    std::env::var("NS_RATCHET_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|ms| *ms > 0)
        .map_or(TIMEOUT, Duration::from_millis)
}

/// The ratchets already run twice tonight, from the file `NS_RATCHET_STEADY` names.
fn steady() -> (Option<PathBuf>, BTreeSet<String>) {
    let Some(path) = std::env::var_os(STEADY_VAR).map(PathBuf::from) else {
        return (None, BTreeSet::new());
    };
    let names = fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    (Some(path), names)
}

/// Write `value` to `path` through a temporary file, so a reader never sees half of it.
fn write_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, serde_json::to_string_pretty(value)? + "\n")
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))
}

/// How `check` and `update` print their result.
#[derive(Clone, Copy)]
pub enum Format {
    Json,
    Human,
}

fn emit(format: Format, v: &Value, outcomes: &[Outcome], summary: &str) -> Result<()> {
    match format {
        Format::Json => println!("{}", serde_json::to_string_pretty(v)?),
        Format::Human => {
            for o in outcomes {
                println!("{}", o.line());
            }
            println!("{summary}");
        }
    }
    Ok(())
}

fn count(outcomes: &[Outcome]) -> String {
    let mut by: BTreeMap<Status, usize> = BTreeMap::new();
    for o in outcomes {
        *by.entry(o.status).or_default() += 1;
    }
    let parts: Vec<String> = by.iter().map(|(s, n)| format!("{n} {s}")).collect();
    match parts.is_empty() {
        true => "0 ratchets".to_string(),
        false => format!("{} ratchets: {}", outcomes.len(), parts.join(", ")),
    }
}

pub fn check(factory: Option<&Path>, format: Format) -> Result<ExitCode> {
    let from_env = std::env::var_os(FACTORY_VAR).map(PathBuf::from);
    let c = ctx(
        factory.map(Path::to_path_buf).or(from_env),
        "ns ratchet check",
    )?;
    let (steady_file, mut steady) = steady();
    let timeout = timeout();
    let outcomes: Vec<Outcome> = c
        .ratchets
        .iter()
        .map(|(name, r)| {
            let rec = c.recorded.values.get(name).copied();
            let runs = match steady.contains(name) {
                true => STEADY_RUNS,
                false => FIRST_CHECK_RUNS,
            };
            assess(name, r, rec, runs, &c.root, timeout)
        })
        .collect();
    let ok = !outcomes.iter().any(Outcome::failed);
    let v = json!({
        "ok": ok,
        "command": "ratchet check",
        "recorded": c.file,
        "ratchets": outcomes,
    });
    if let Some(p) = std::env::var_os(REPORT_VAR) {
        write_atomic(Path::new(&p), &v)?;
    }
    if let Some(path) = steady_file {
        let ran_twice = outcomes
            .iter()
            .filter(|o| o.runs.len() == 2 && o.status != Status::Flaky)
            .map(|o| o.name.clone());
        let before = steady.len();
        steady.extend(ran_twice);
        if steady.len() != before {
            write_atomic(&path, &steady)?;
        }
    }
    emit(format, &v, &outcomes, &count(&outcomes))?;
    if outcomes.iter().any(|o| o.status == Status::Untracked) {
        eprintln!("ns ratchet: untracked ratchets have no recorded value; a human records them with:\n  ns ratchet update");
    }
    if !ok {
        eprintln!(
            "ns ratchet: a ratchet failed. Get each worse value back within its limit and make each command print one steady number, then rerun:\n  ns ratchet check\nNever edit {FILE} or a [ratchets.*] table to pass: loosening a ratchet is a human decision."
        );
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

pub fn update(
    factory: Option<&Path>,
    allow_loosen: bool,
    dry_run: bool,
    format: Format,
) -> Result<ExitCode> {
    let c = ctx(factory.map(Path::to_path_buf), "ns ratchet update")?;
    if c.ratchets.is_empty() {
        return Err(SfError::usage(
            "no [ratchets.*] table in nightshift.toml",
            "add one, then rerun:\n  [ratchets.flash]\n  command = \"scripts/flash-size.sh\"\n  better = \"lower\"\n  tolerance = \"0.5%\"\n\n  ns ratchet update",
        )
        .into());
    }
    let timeout = timeout();
    let mut outcomes = Vec::new();
    let mut values = BTreeMap::new();
    for (name, r) in &c.ratchets {
        let rec = c.recorded.values.get(name).copied();
        let mut o = assess(name, r, rec, FIRST_CHECK_RUNS, &c.root, timeout);
        let (status, value) = match (o.status, o.measured, rec) {
            (Status::Broken | Status::Flaky, ..) | (_, None, _) => (o.status, None),
            (_, Some(m), None) => (Status::Recorded, Some(m)),
            (_, Some(m), Some(r0)) if m == r0 => (Status::Unchanged, Some(m)),
            (_, Some(m), Some(r0)) if !worse(r.better, m, r0) => (Status::Tightened, Some(m)),
            (_, Some(m), Some(_)) if allow_loosen => (Status::Loosened, Some(m)),
            (_, Some(_), Some(r0)) => (Status::Kept, Some(r0)),
        };
        o.status = status;
        if let Some(v) = value {
            values.insert(name.clone(), v);
        }
        outcomes.push(o);
    }
    let refused = outcomes
        .iter()
        .any(|o| matches!(o.status, Status::Broken | Status::Flaky));
    let dropped: Vec<&String> = c
        .recorded
        .values
        .keys()
        .filter(|k| !c.ratchets.contains_key(*k))
        .collect();
    let status = if refused {
        "refused"
    } else if values == c.recorded.values {
        "already-done"
    } else if dry_run {
        "planned"
    } else {
        write_atomic(
            &c.file,
            &Recorded {
                version: 1,
                values: values.clone(),
            },
        )?;
        "written"
    };
    let v = json!({
        "ok": !refused,
        "command": "ratchet update",
        "status": status,
        "dry_run": dry_run,
        "recorded": c.file,
        "ratchets": outcomes,
        "dropped": dropped,
    });
    emit(
        format,
        &v,
        &outcomes,
        &format!("{status}: {}", count(&outcomes)),
    )?;
    if outcomes.iter().any(|o| o.status == Status::Kept) {
        eprintln!("ns ratchet: worse ratchets keep their recorded value; a human loosens them with:\n  ns ratchet update --allow-loosen");
    }
    if refused {
        eprintln!(
            "ns ratchet: nothing written: a ratchet is broken or flaky. Fix its command, then rerun:\n  ns ratchet update"
        );
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

/// What a `ns ratchet check` report says failed, one line per failed ratchet; none when the
/// report is missing (the check didn't run) or names no failure.
pub fn failures(report: &Path) -> Option<String> {
    let v: Value = serde_json::from_str(&fs::read_to_string(report).ok()?).ok()?;
    let lines: Vec<String> = v["ratchets"]
        .as_array()?
        .iter()
        .filter_map(|o| serde_json::from_value::<Outcome>(o.clone()).ok())
        .filter(Outcome::failed)
        .map(|o| o.line())
        .collect();
    (!lines.is_empty()).then(|| lines.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::Tolerance;

    fn tol(toml: &str) -> Result<Tolerance, String> {
        #[derive(Deserialize)]
        struct T {
            tolerance: Tolerance,
        }
        toml::from_str::<T>(&format!("tolerance = {toml}"))
            .map(|t| t.tolerance)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn a_tolerance_is_a_percentage_or_a_number() {
        assert_eq!(tol("\"0.5%\""), Ok(Tolerance::Percent(0.5)));
        assert_eq!(tol("\" 2 % \""), Ok(Tolerance::Percent(2.0)));
        assert_eq!(tol("1024"), Ok(Tolerance::Absolute(1024.0)));
        assert_eq!(tol("0.25"), Ok(Tolerance::Absolute(0.25)));
        assert_eq!(tol("\"3\""), Ok(Tolerance::Absolute(3.0)));
        assert_eq!(tol("0"), Ok(Tolerance::Absolute(0.0)));
        for bad in ["\"-1%\"", "-0.5", "\"%\"", "\"x\"", "inf", "nan", "true"] {
            assert!(tol(bad).is_err(), "{bad}");
        }
        assert!(tol("\"-1%\"").unwrap_err().contains("not negative"));
        assert_eq!(Tolerance::default(), Tolerance::Absolute(0.0));
        assert_eq!(Tolerance::Percent(0.5).to_string(), "0.5%");
        assert_eq!(Tolerance::Absolute(3.0).to_string(), "3");
    }

    #[test]
    fn the_limit_moves_the_recorded_value_by_the_tolerance_the_worse_way() {
        let r = |better, tolerance| Ratchet {
            command: String::new(),
            better,
            tolerance,
        };
        assert_eq!(
            limit(&r(Better::Lower, Tolerance::Percent(10.0)), 200.0),
            220.0
        );
        assert_eq!(
            limit(&r(Better::Higher, Tolerance::Percent(10.0)), 200.0),
            180.0
        );
        assert_eq!(
            limit(&r(Better::Lower, Tolerance::Absolute(3.0)), 200.0),
            203.0
        );
        assert_eq!(
            limit(&r(Better::Higher, Tolerance::Percent(10.0)), -200.0),
            -220.0
        );
        assert!(worse(Better::Lower, 2.0, 1.0) && !worse(Better::Lower, 1.0, 1.0));
        assert!(worse(Better::Higher, 1.0, 2.0) && !worse(Better::Higher, 2.0, 2.0));
    }

    #[test]
    fn failures_read_only_failed_ratchets_from_a_well_formed_report() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("r.json");
        assert_eq!(failures(&p), None);
        for bad in [
            "not json",
            "{}",
            r#"{"ratchets": {}}"#,
            r#"{"ratchets": [{"name": 1}]}"#,
        ] {
            fs::write(&p, bad).unwrap();
            assert_eq!(failures(&p), None, "{bad}");
        }
        let o = |name: &str, status: &str| {
            json!({"name": name, "command": "c", "better": "lower", "tolerance": "0",
                   "recorded": null, "measured": null, "runs": [1, 2], "limit": null,
                   "status": status, "error": "exited 2"})
        };
        let v = json!({"ratchets": [o("a", "ok"), o("b", "broken"), o("c", "flaky"), o("d", "untracked")]});
        fs::write(&p, json!({"ratchets": [o("a", "ok")]}).to_string()).unwrap();
        assert_eq!(failures(&p), None);
        fs::write(&p, v.to_string()).unwrap();
        assert_eq!(
            failures(&p).as_deref(),
            Some("ratchet b is broken: `c` exited 2; ratchet c is flaky: two runs of `c` measured 1 and 2, more apart than its tolerance 0")
        );
    }

    #[test]
    fn a_status_prints_as_its_json_name() {
        assert_eq!(Status::Untracked.to_string(), "untracked");
        assert_eq!(Status::Kept.to_string(), "kept");
    }
}
