//! The opt-in memory cap, `[limits] memory_mb` (docs/factory/run.md, "Memory cap"). Phases and
//! the gate start under it: in a systemd user scope with `MemoryMax` where that works, else
//! under `prlimit --as`.

use std::ffi::OsStr;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::error::SfError;
use crate::proc::{Kind, Proc};

const MIB: u64 = 1024 * 1024;
/// How long a probe may take before it counts as failed.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for systemd to settle a scope after its process exits.
const SETTLE: Duration = Duration::from_secs(3);
/// How long a dropped scope may take to stop before it is reset anyway.
const RELEASE: Duration = Duration::from_secs(10);
/// The states of a scope systemd has not settled yet.
const SETTLING: &[&str] = &["active", "activating", "deactivating", "reloading"];
/// Messages runtimes print when an allocation fails, each matched within one line. Under
/// RLIMIT_AS nothing is killed at the cap; the allocation fails and the program reports it.
const ALLOC_FAILED: &[&str] = &[
    "std::bad_alloc",
    "Cannot allocate memory",
    "fatal error: runtime: out of memory",
    "Fatal process out of memory",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// A systemd user scope with `MemoryMax`: the kernel kills the scope at the cap.
    Cgroup,
    /// `prlimit --as`: allocations past the cap fail.
    Rlimit,
}

impl Via {
    pub fn label(self) -> &'static str {
        match self {
            Via::Cgroup => "systemd-run",
            Via::Rlimit => "prlimit",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cap {
    pub mb: u64,
    pub via: Via,
    /// Why the cgroup was not used, when it wasn't.
    pub fallback: Option<String>,
}

/// How a capped run is told apart afterwards.
pub enum Watch {
    None,
    Scope(Scope),
    Rlimit { mb: u64 },
}

/// A systemd scope a run starts in. Dropping it waits for the scope to stop and clears it, on
/// every path, so a failed one doesn't pile up in `systemctl --user --failed`.
pub struct Scope {
    unit: String,
    mb: u64,
}

impl Drop for Scope {
    fn drop(&mut self) {
        settle(&self.unit, RELEASE);
        let _ = Proc::tool("systemctl")
            .args(["--user", "reset-failed", &self.unit])
            .output();
    }
}

/// "exceeded the 64 MB memory limit".
pub fn exceeded_reason(mb: u64) -> String {
    format!("exceeded the {mb} MB memory limit")
}

/// Why a run was cut short by something other than `ns`: the memory cap or a signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// Over the cap of this many MB.
    Exceeded(u64),
    /// Killed by this signal.
    Signal(i32),
}

impl Cut {
    /// "exceeded the 64 MB memory limit" or "was killed by SIGKILL".
    pub fn reason(self) -> String {
        match self {
            Cut::Exceeded(mb) => exceeded_reason(mb),
            Cut::Signal(sig) => format!("was killed by {}", crate::stop::name(sig)),
        }
    }
}

/// The cap for `memory_mb`: the cgroup when a probe shows it enforces `MemoryMax`, else
/// RLIMIT_AS when `prlimit` runs, else an error.
pub fn detect(mb: u64) -> anyhow::Result<Cap> {
    let why = match probe_cgroup(mb) {
        Ok(()) => {
            return Ok(Cap {
                mb,
                via: Via::Cgroup,
                fallback: None,
            })
        }
        Err(why) => why,
    };
    match probe_rlimit(mb) {
        Ok(()) => Ok(Cap {
            mb,
            via: Via::Rlimit,
            fallback: Some(why),
        }),
        Err(why2) => Err(SfError::usage(
            format!("limits.memory_mb = {mb}, but no memory cap works here: {why}; {why2}"),
            "install systemd (a user session) or util-linux's prlimit, or remove memory_mb from\n  .nightshift/nightshift.toml",
        )
        .into()),
    }
}

/// A `kind` child that runs `program` under `cap`, and how to tell afterwards whether it hit
/// the cap. The caller adds the program's arguments.
pub fn command(cap: Option<&Cap>, kind: Kind, program: impl AsRef<OsStr>) -> (Proc, Watch) {
    wrapped(cap, kind, &[], program)
}

/// As [`command`], with `program` started through `inner`, a launcher of its own, inside the cap.
pub fn wrapped(
    cap: Option<&Cap>,
    kind: Kind,
    inner: &[String],
    program: impl AsRef<OsStr>,
) -> (Proc, Watch) {
    let (mut launcher, watch) = launcher(cap);
    launcher.extend_from_slice(inner);
    (Proc::launched(kind, &launcher, program), watch)
}

/// The command that starts a program under `cap`, empty without one.
fn launcher(cap: Option<&Cap>) -> (Vec<String>, Watch) {
    let Some(cap) = cap else {
        return (Vec::new(), Watch::None);
    };
    match cap.via {
        Via::Cgroup => {
            let unit = scope_name();
            let mut c = vec!["systemd-run".to_string()];
            c.extend(scope_args(&unit, cap.mb));
            (c, Watch::Scope(Scope { unit, mb: cap.mb }))
        }
        Via::Rlimit => (
            vec![
                "prlimit".to_string(),
                format!("--as={}", cap.mb * MIB),
                "--".to_string(),
            ],
            Watch::Rlimit { mb: cap.mb },
        ),
    }
}

fn scope_args(unit: &str, mb: u64) -> Vec<String> {
    [
        "--user",
        "--scope",
        "--quiet",
        &format!("--unit={unit}"),
        "-p",
        &format!("MemoryMax={mb}M"),
        "-p",
        "MemorySwapMax=0",
        "-p",
        "OOMPolicy=stop",
        "--",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// A scope name no other run uses: this process, a counter and the time.
fn scope_name() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "ns-{}-{}-{nanos}.scope",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    )
}

impl Watch {
    /// How a finished run was cut short, if it was: `status` is `None` when `ns` killed it at
    /// its timeout, and `output` is the end of what it printed. The cap wins over the signal
    /// that enforced it, and a timeout is neither: `ns` sent that kill.
    pub fn cut_short(
        &self,
        status: Option<ExitStatus>,
        timed_out: bool,
        output: impl FnOnce() -> String,
    ) -> Option<Cut> {
        if timed_out {
            return None;
        }
        let failed = !status.is_some_and(|s| s.success());
        self.exceeded(failed, output)
            .map(Cut::Exceeded)
            .or_else(|| status.and_then(signal_of).map(Cut::Signal))
    }

    /// The cap a finished run went over, if it did. `failed` is whether it exited non-zero or
    /// by a signal. A scope asks systemd, which records an OOM kill as the scope's result.
    /// Under RLIMIT_AS nothing is killed, so a failed run counts when its output holds an
    /// allocation-failure message.
    fn exceeded(&self, failed: bool, output: impl FnOnce() -> String) -> Option<u64> {
        match self {
            Watch::None => None,
            Watch::Scope(s) => oom_killed(&s.unit).then_some(s.mb),
            Watch::Rlimit { mb } => (failed && allocation_failed(&output())).then_some(*mb),
        }
    }
}

/// The signal that ended a process, if one did.
fn signal_of(status: ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

/// Whether a line of `output` is an allocation failure: Rust's "memory allocation of <n>
/// bytes failed", Python's `MemoryError`, or one of [`ALLOC_FAILED`].
fn allocation_failed(output: &str) -> bool {
    output.lines().any(|l| {
        (l.contains("memory allocation of ") && l.trim_end().ends_with(" bytes failed"))
            || l.trim_start().starts_with("MemoryError")
            || ALLOC_FAILED.iter().any(|m| l.contains(m))
    })
}

/// Whether systemd ended `unit` with `oom-kill`, waiting for the scope to stop.
fn oom_killed(unit: &str) -> bool {
    settle(unit, SETTLE) == "oom-kill"
}

/// Ask systemd about `unit` until it stops or `wait` passes; its last result.
fn settle(unit: &str, wait: Duration) -> String {
    let deadline = Instant::now() + wait;
    loop {
        let (state, result) = scope_state(unit);
        if !SETTLING.contains(&state.as_str()) || Instant::now() >= deadline {
            return result;
        }
        std::thread::sleep(Duration::ZERO);
    }
}

fn scope_state(unit: &str) -> (String, String) {
    let out = Proc::tool("systemctl")
        .args(["--user", "show", unit, "--property=ActiveState,Result"])
        .output();
    let text = out
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
            .unwrap_or("")
            .trim()
            .to_string()
    };
    (get("ActiveState"), get("Result"))
}

/// Ok when a scope started with the same properties as a real run (so a systemd that rejects
/// one falls back here, not in every phase) gets `memory.max` set to the cap: the command
/// reads its own cgroup's limit back.
fn probe_cgroup(mb: u64) -> Result<(), String> {
    let mut c = Proc::tool("systemd-run");
    c.arg("--collect")
        .args(scope_args(&scope_name(), mb))
        .args([
            "sh",
            "-c",
            r#"cat "/sys/fs/cgroup$(sed -n 's/^0:://p' /proc/self/cgroup)/memory.max""#,
        ]);
    let out = probe(c, "systemd-run", PROBE_TIMEOUT)?;
    let want = (mb * MIB).to_string();
    if out.trim() == want {
        Ok(())
    } else {
        Err(format!(
            "systemd-run --user did not set the scope's memory.max to {want} (read {:?})",
            out.trim()
        ))
    }
}

/// Ok when `prlimit --as` can start a command.
fn probe_rlimit(mb: u64) -> Result<(), String> {
    let mut c = Proc::tool("prlimit");
    c.arg(format!("--as={}", mb * MIB)).args(["--", "true"]);
    probe(c, "prlimit", PROBE_TIMEOUT).map(|_| ())
}

/// Run a probe; its stdout, or why it failed.
pub(crate) fn probe(mut c: Proc, name: &str, timeout: Duration) -> Result<String, String> {
    match c.timeout(timeout).output() {
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => Err(format!("{name} failed ({})", out.status)),
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Err(format!("{name} timed out")),
        Err(e) => Err(format!("{name}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    #[test]
    fn a_probe_returns_its_stdout_or_says_how_it_failed() {
        let sh = |script: &str| {
            let mut p = Proc::tool("sh");
            p.args(["-c", script]);
            probe(p, "sh", PROBE_TIMEOUT)
        };
        assert_eq!(sh("echo 1024").unwrap(), "1024\n");
        let failed = sh("echo no >&2; exit 3").unwrap_err();
        assert!(failed.starts_with("sh failed (exit status: 3)"), "{failed}");
        let missing = probe(
            Proc::tool("/nonexistent/ns-memcap-probe"),
            "probe",
            PROBE_TIMEOUT,
        )
        .unwrap_err();
        assert!(missing.starts_with("probe: "), "{missing}");
    }

    #[test]
    fn a_probe_that_outlives_its_timeout_says_it_timed_out() {
        let mut p = Proc::tool("sh");
        p.args(["-c", "exec sleep 600"]);
        let err = probe(p, "sh", Duration::ZERO).unwrap_err();
        assert_eq!(err, "sh timed out");
    }

    #[test]
    fn no_cap_runs_the_program_itself() {
        let (c, w) = command(None, Kind::Tool, "claude");
        assert_eq!(c.argv(), ["claude"]);
        assert!(matches!(w, Watch::None));
    }

    fn cap(via: Via) -> Cap {
        Cap {
            mb: 64,
            via,
            fallback: None,
        }
    }

    #[test]
    fn the_cgroup_runs_the_program_in_a_named_scope_with_memory_max() {
        let (mut c, w) = command(Some(&cap(Via::Cgroup)), Kind::Tool, "claude");
        c.arg("-p");
        let Watch::Scope(Scope { unit, mb }) = &w else {
            panic!("not a scope")
        };
        assert_eq!(*mb, 64);
        assert!(
            unit.starts_with("ns-") && unit.ends_with(".scope"),
            "{unit}"
        );
        assert_eq!(
            c.argv(),
            [
                "systemd-run",
                "--user",
                "--scope",
                "--quiet",
                &format!("--unit={unit}"),
                "-p",
                "MemoryMax=64M",
                "-p",
                "MemorySwapMax=0",
                "-p",
                "OOMPolicy=stop",
                "--",
                "claude",
                "-p"
            ]
        );
        let (_, again) = command(Some(&cap(Via::Cgroup)), Kind::Tool, "claude");
        let Watch::Scope(Scope { unit: other, .. }) = &again else {
            panic!()
        };
        assert_ne!(unit, other);
    }

    #[test]
    fn the_rlimit_runs_the_program_under_prlimit_as() {
        let (mut c, w) = command(Some(&cap(Via::Rlimit)), Kind::Tool, "sh");
        c.args(["-c", "true"]);
        assert!(matches!(w, Watch::Rlimit { mb: 64 }));
        assert_eq!(
            c.argv(),
            ["prlimit", "--as=67108864", "--", "sh", "-c", "true"]
        );
    }

    #[test]
    fn an_rlimit_run_exceeded_only_when_it_failed_with_an_allocation_message() {
        let w = Watch::Rlimit { mb: 64 };
        for msg in [
            "memory allocation of 268435456 bytes failed",
            "Traceback\nMemoryError",
            "  MemoryError: out of space",
            "terminate called after throwing an instance of 'std::bad_alloc'",
            "sh: fork: Cannot allocate memory",
            "fatal error: runtime: out of memory",
            "# Fatal process out of memory: Zone",
        ] {
            assert_eq!(w.exceeded(true, || msg.into()), Some(64), "{msg}");
            assert_eq!(w.exceeded(false, || msg.into()), None, "{msg}");
        }
        for msg in [
            "assertion failed",
            "test tests::out_of_memory_is_reported ... FAILED",
            "handles MemoryError gracefully: FAILED",
            "memory allocation of a buffer: ok",
        ] {
            assert_eq!(w.exceeded(true, || msg.into()), None, "{msg}");
        }
        assert_eq!(Watch::None.exceeded(true, || "MemoryError".into()), None);
    }

    #[test]
    fn a_scope_systemd_does_not_know_did_not_exceed() {
        let w = Watch::Scope(Scope {
            unit: "ns-never-started-0-0.scope".into(),
            mb: 64,
        });
        let t = Instant::now();
        assert_eq!(w.exceeded(true, || "MemoryError".into()), None);
        assert!(t.elapsed() < SETTLE, "{:?}", t.elapsed());
    }

    fn exit(code: i32) -> Option<ExitStatus> {
        use std::os::unix::process::ExitStatusExt;
        Some(ExitStatus::from_raw(code << 8))
    }

    fn signalled(sig: i32) -> Option<ExitStatus> {
        use std::os::unix::process::ExitStatusExt;
        Some(ExitStatus::from_raw(sig))
    }

    #[test]
    fn cut_short_names_the_cap_over_the_signal_and_neither_after_a_timeout() {
        let w = Watch::Rlimit { mb: 64 };
        let oom = || "MemoryError".to_string();
        assert_eq!(w.cut_short(exit(1), false, oom), Some(Cut::Exceeded(64)));
        assert_eq!(
            w.cut_short(signalled(libc::SIGABRT), false, || {
                "memory allocation of 8 bytes failed".into()
            }),
            Some(Cut::Exceeded(64))
        );
        assert_eq!(
            w.cut_short(signalled(libc::SIGKILL), false, String::new),
            Some(Cut::Signal(libc::SIGKILL))
        );
        assert_eq!(w.cut_short(None, true, oom), None);
        assert_eq!(w.cut_short(exit(1), false, String::new), None);
        assert_eq!(w.cut_short(exit(0), false, oom), None);
        assert_eq!(
            Watch::None.cut_short(signalled(libc::SIGSEGV), false, oom),
            Some(Cut::Signal(libc::SIGSEGV))
        );
        assert_eq!(
            Cut::Exceeded(64).reason(),
            "exceeded the 64 MB memory limit"
        );
        assert_eq!(Cut::Signal(libc::SIGKILL).reason(), "was killed by SIGKILL");
    }

    /// Run `python3` allocating `alloc_mb` under a 64 MB cap by `via`; what `cut_short` says.
    fn hog(via: Via, alloc_mb: u64) -> (Option<Cut>, Option<ExitStatus>) {
        let (r, st, _) = hog_watched(via, alloc_mb);
        (r, st)
    }

    fn hog_watched(via: Via, alloc_mb: u64) -> (Option<Cut>, Option<ExitStatus>, Watch) {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("out");
        let c = cap(via);
        let (mut cmd, w) = command(Some(&c), Kind::Tool, "python3");
        cmd.args([
            "-c",
            &format!("b = b'x' * ({alloc_mb} << 20); print('survived')"),
        ])
        .output_to(fs::File::create(&log).unwrap())
        .stderr_to(fs::File::create(tmp.path().join("err")).unwrap());
        let ended = cmd.run().unwrap();
        assert!(!ended.timed_out());
        let st = ended.status;
        let err = tmp.path().join("err");
        (
            w.cut_short(st, false, || fs::read_to_string(&err).unwrap()),
            st,
            w,
        )
    }

    #[test]
    fn rlimit_catches_an_allocation_past_the_cap_and_passes_one_under_it() {
        if probe_rlimit(64).is_err() {
            eprintln!("skipped: prlimit does not work here");
            return;
        }
        let (r, _) = hog(Via::Rlimit, 256);
        assert_eq!(r, Some(Cut::Exceeded(64)));
        let (r, st) = hog(Via::Rlimit, 8);
        assert_eq!(r, None);
        assert!(st.unwrap().success());
    }

    #[test]
    fn the_cgroup_kills_an_allocation_past_the_cap_and_passes_one_under_it() {
        if let Err(why) = probe_cgroup(64) {
            eprintln!("skipped: {why}");
            return;
        }
        let (r, st, w) = hog_watched(Via::Cgroup, 256);
        assert_eq!(r, Some(Cut::Exceeded(64)));
        assert!(!st.unwrap().success());
        let Watch::Scope(Scope { unit, .. }) = &w else {
            panic!("not a scope")
        };
        let unit = unit.clone();
        drop(w);
        assert_ne!(scope_state(&unit).0, "failed", "{unit} was left failed");
        let (r, st) = hog(Via::Cgroup, 8);
        assert_eq!(r, None);
        assert!(st.unwrap().success());
    }

    #[test]
    fn detect_prefers_the_cgroup_when_it_works() {
        let cap = detect(64).unwrap();
        match probe_cgroup(64) {
            Ok(()) => {
                assert_eq!(cap.via, Via::Cgroup);
                assert_eq!(cap.fallback, None);
            }
            Err(why) => {
                assert_eq!(cap.via, Via::Rlimit);
                assert_eq!(cap.fallback, Some(why));
            }
        }
        assert_eq!(cap.mb, 64);
    }

    #[test]
    fn exceeded_reason_names_the_cap() {
        assert_eq!(exceeded_reason(2048), "exceeded the 2048 MB memory limit");
    }
}
