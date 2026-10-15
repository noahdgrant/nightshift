//! The subprocess guard: every `ns` a test starts is bounded, and its process tree is killed.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::Group;

fn ns() -> common::Ns {
    let mut c = common::ns();
    c.env_remove("NS_CONFIG").env_remove("XDG_CONFIG_HOME");
    c
}

/// A job for a test's process to start in the background, which lives past
/// [`common::TIMEOUT`] unless it is killed. It holds the fifo `held` open while it lives, so the
/// test learns it died when a read of `held` ends.
struct Grandchild {
    held: PathBuf,
    ready: PathBuf,
    closed: mpsc::Receiver<()>,
}

impl Grandchild {
    fn new(dir: &Path) -> Grandchild {
        let (held, ready) = (dir.join("held"), dir.join("ready"));
        for f in [&held, &ready] {
            assert!(StdCommand::new("mkfifo").arg(f).status().unwrap().success());
        }
        let (tx, closed) = mpsc::channel();
        let f = held.clone();
        std::thread::spawn(move || {
            let mut rest = Vec::new();
            fs::File::open(f).unwrap().read_to_end(&mut rest).unwrap();
            let _ = tx.send(());
        });
        Grandchild {
            held,
            ready,
            closed,
        }
    }

    /// Shell for the job, to run in the background.
    fn job(&self) -> String {
        format!(
            "(echo up > {:?}; exec sleep 600) 3>{:?}",
            self.ready, self.held
        )
    }

    /// Shell that waits until the job holds `held`.
    fn up(&self) -> String {
        format!("read up < {:?}", self.ready)
    }

    /// Every holder of `held` closed it, so the job is dead.
    fn died(&self) -> bool {
        self.closed.recv_timeout(common::TIMEOUT).is_ok()
    }
}

fn harness_config(dir: &Path, script: &str) -> PathBuf {
    let cfg = dir.join("config.toml");
    fs::write(
        &cfg,
        format!("[harness.h]\ncommand = [\"sh\", \"-c\", '''{script}''']\n[roles.review]\nharness = \"h\"\n"),
    )
    .unwrap();
    cfg
}

#[test]
fn a_hung_ns_fails_at_its_timeout_and_takes_its_children_with_it() {
    let tmp = tempfile::tempdir().unwrap();
    let job = Grandchild::new(tmp.path());
    let cfg = harness_config(
        tmp.path(),
        &format!("{} & {}; sleep 30", job.job(), job.up()),
    );
    let panic = std::panic::catch_unwind(|| {
        ns().env("NS_CONFIG", &cfg)
            .args(["ask", "--role", "review"])
            .write_stdin("x")
            .timeout(Duration::from_secs(3))
            .output()
    })
    .unwrap_err();
    let msg = panic.downcast_ref::<String>().unwrap();
    assert!(msg.contains("timed out after 3s"), "{msg}");
    assert!(msg.contains("\"ask\" \"--role\" \"review\""), "{msg}");
    assert!(job.died(), "the harness's background job outlived ns");
}

#[test]
fn a_started_run_that_never_finishes_fails_at_its_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = harness_config(tmp.path(), "sleep 30");
    let prompt = tmp.path().join("prompt.md");
    fs::write(&prompt, "x").unwrap();
    let mut run = ns()
        .env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review", "--prompt-file"])
        .arg(&prompt)
        .timeout(Duration::from_secs(3))
        .start();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.wait())).unwrap_err();
    let msg = panic.downcast_ref::<String>().unwrap();
    assert!(msg.contains("timed out after 3s"), "{msg}");
    assert!(msg.contains("\"ask\" \"--role\" \"review\""), "{msg}");
}

#[test]
fn waiting_for_an_event_that_never_comes_fails_at_the_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = harness_config(tmp.path(), "sleep 30");
    let prompt = tmp.path().join("prompt.md");
    fs::write(&prompt, "x").unwrap();
    let run = ns()
        .env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review", "--prompt-file"])
        .arg(&prompt)
        .timeout(Duration::from_secs(3))
        .start();
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    let begun = Instant::now();
    let panic =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.recv(&rx))).unwrap_err();
    assert!(begun.elapsed() < Duration::from_secs(15));
    let msg = panic.downcast_ref::<String>().unwrap();
    assert!(msg.contains("timed out after 3s"), "{msg}");
}

#[test]
fn exits_counts_a_killed_process_as_exited_whether_or_not_it_was_reaped() {
    let mut sleeper = Group::spawn(
        StdCommand::new("sh").args(["-c", "exec sleep 600"]),
        common::TIMEOUT,
    );
    let pid = sleeper.id().to_string();
    sleeper.kill();
    assert!(common::exits(&pid), "a zombie");
    sleeper.wait();
    assert!(common::exits(&pid), "a reaped pid");
}

#[test]
fn dropping_a_group_kills_its_grandchildren() {
    let tmp = tempfile::tempdir().unwrap();
    let job = Grandchild::new(tmp.path());
    let mut sh = StdCommand::new("sh");
    sh.arg("-c")
        .arg(format!(
            "{} & {}; echo started; sleep 30",
            job.job(),
            job.up()
        ))
        .stdout(Stdio::piped());
    let mut group = Group::spawn(&mut sh, common::TIMEOUT);
    let mut line = String::new();
    BufReader::new(group.take_stdout())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line, "started\n");
    drop(group);
    assert!(job.died(), "the background job outlived the group");
}

const BANNED: [&str; 4] = [
    concat!("assert", "_cmd"),
    concat!("CARGO_BIN", "_EXE"),
    concat!("cargo", "_bin"),
    concat!(".spawn", "()"),
];

fn unbounded_starts(text: &str) -> Vec<(usize, &'static str)> {
    lines_with(text, &BANNED)
}

/// What makes a file executable by hand, which only `write_exe` may do in a test.
const EXE_WRITES: [&str; 3] = [
    concat!("0o", "755"),
    concat!("Command::new(\"", "chmod\")"),
    concat!(".arg(\"+", "x\")"),
];

fn lines_with(text: &str, patterns: &[&'static str]) -> Vec<(usize, &'static str)> {
    text.lines()
        .enumerate()
        .filter_map(|(n, line)| {
            patterns
                .iter()
                .find(|b| line.contains(**b))
                .map(|b| (n + 1, *b))
        })
        .collect()
}

fn rust_files_outside_common(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_none_or(|n| n != "common") {
                rust_files_outside_common(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_guard_flags_a_process_started_without_a_timeout() {
    let hits = unbounded_starts(&format!(
        "fn ok() {{}}\nlet c = {}::Command::new(\"ns\");\nlet d = env!(\"{}_ns\");\n",
        BANNED[0], BANNED[1]
    ));
    assert_eq!(hits, vec![(2, BANNED[0]), (3, BANNED[1])]);
    assert!(unbounded_starts("let ns = common::ns();\nns.start();\n").is_empty());
}

#[test]
fn every_test_starts_ns_through_the_bounded_helper() {
    let mut files = Vec::new();
    rust_files_outside_common(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"),
        &mut files,
    );
    assert!(files.len() >= 4, "scanned {files:?}");
    for path in files {
        let text = fs::read_to_string(&path).unwrap();
        if let Some((n, b)) = unbounded_starts(&text).first() {
            panic!(
                "{}:{n}: `{b}` starts a process without a timeout; use common::ns() or common::Group::spawn()",
                path.display()
            );
        }
    }
}

/// A test that writes an executable itself and then execs it fails with ETXTBSY when another
/// test thread forks in between, so every one goes through `write_exe` (`tests/common`, or
/// `testutil` for the unit tests in `src`).
#[test]
fn every_test_writes_executables_through_write_exe() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files_outside_common(&manifest.join("tests"), &mut files);
    rust_files_outside_common(&manifest.join("src"), &mut files);
    files.retain(|p| !p.ends_with("src/testutil.rs"));
    assert!(files.len() >= 20, "scanned {files:?}");
    for path in files {
        let text = fs::read_to_string(&path).unwrap();
        if let Some((n, b)) = lines_with(&text, &EXE_WRITES).first() {
            panic!(
                "{}:{n}: `{b}` makes a file executable by hand; use common::write_exe or testutil::write_exe",
                path.display()
            );
        }
    }
}

#[test]
fn output_returns_when_ns_exits_even_if_its_child_holds_the_pipes() {
    let tmp = tempfile::tempdir().unwrap();
    let job = Grandchild::new(tmp.path());
    let cfg = harness_config(
        tmp.path(),
        &format!("{} & {}; echo hi", job.job(), job.up()),
    );
    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review"])
        .write_stdin("x")
        .assert()
        .success()
        .stdout("hi\n");
    assert!(job.died(), "the harness's background job outlived ns");
}

#[test]
fn output_kills_a_descendant_that_left_the_process_group_and_outlived_its_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let job = Grandchild::new(tmp.path());
    let script = format!("setsid sh -c '{}' & {}; echo hi", job.job(), job.up());
    let cfg = harness_config(tmp.path(), &script);
    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review"])
        .write_stdin("x")
        .assert()
        .success()
        .stdout("hi\n");
    assert!(job.died(), "the detached job outlived ns");
}

/// `clippy.toml` bans `Command::new` and `Proc::harness`: clippy, with this crate's config,
/// rejects a stray call of each in a scratch crate named `ns`, with a `proc::Proc::harness` of its
/// own, and names the way to start each.
#[test]
fn clippy_rejects_a_child_started_outside_proc_or_harness() {
    let tmp = tempfile::tempdir().unwrap();
    let krate = tmp.path().join("ns");
    fs::create_dir_all(krate.join("src")).unwrap();
    fs::write(
        krate.join("Cargo.toml"),
        "[package]\nname = \"ns\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[workspace]\n",
    )
    .unwrap();
    fs::write(
        krate.join("src/main.rs"),
        "mod proc {\n    pub struct Proc;\n    impl Proc {\n        pub fn harness() {}\n    }\n}\n\nfn main() {\n    let _ = std::process::Command::new(\"true\");\n    proc::Proc::harness();\n}\n",
    )
    .unwrap();
    let out = common::command(env!("CARGO"))
        .args(["clippy", "--quiet", "--offline", "--manifest-path"])
        .arg(krate.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(tmp.path().join("target"))
        .args(["--", "-D", "warnings"])
        .env("CLIPPY_CONF_DIR", env!("CARGO_MANIFEST_DIR"))
        .output();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "clippy passed a stray call: {stderr}"
    );
    assert!(stderr.contains("disallowed_methods"), "{stderr}");
    assert!(stderr.contains("crate::proc::Proc"), "{stderr}");
    assert!(stderr.contains("ns::proc::Proc::harness"), "{stderr}");
    assert!(stderr.contains("crate::harness::command"), "{stderr}");
}

/// Where `src` lifts the ban: `proc`, the one way to start a child, `harness`, the one way to
/// start a harness, and the unit tests' probe helper, which hands its caller a process to spawn
/// and kill (CODING_STANDARDS.md, "Checks").
#[test]
fn only_proc_harness_and_the_probe_helper_lift_the_ban_in_src() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files_outside_common(&manifest.join("src"), &mut files);
    let mut lifted: Vec<String> = files
        .iter()
        .filter(|p| {
            fs::read_to_string(p)
                .unwrap()
                .contains(concat!("clippy::disallowed", "_methods"))
        })
        .map(|p| p.strip_prefix(manifest).unwrap().display().to_string())
        .collect();
    lifted.sort();
    assert_eq!(lifted, ["src/harness.rs", "src/proc.rs", "src/testutil.rs"]);
}
