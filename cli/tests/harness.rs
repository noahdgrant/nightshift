mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

use common::Group;

fn ns() -> common::Ns {
    let mut c = common::ns();
    c.env_remove("NS_CONFIG").env_remove("XDG_CONFIG_HOME");
    c
}

const GRANDCHILD_DELAY: u64 = 2;
const HUNG_GRANDCHILD_DELAY: u64 = 5;

/// A shell that starts a background job which would create `marker` after
/// [`GRANDCHILD_DELAY`] s, reports it started, then sleeps.
fn leaves_a_grandchild(marker: &Path, delay: u64) -> String {
    format!("(sleep {delay}; touch {marker:?}) & echo started; sleep 30")
}

fn survivor_check_wait(delay: u64) -> Duration {
    Duration::from_secs(delay) + Duration::from_millis(500)
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
    let marker = tmp.path().join("survivor");
    let started = tmp.path().join("started");
    let script = format!(
        "touch {started:?}; {}",
        leaves_a_grandchild(&marker, HUNG_GRANDCHILD_DELAY)
    );
    let cfg = harness_config(tmp.path(), &script);
    let begun = Instant::now();
    let panic = std::panic::catch_unwind(|| {
        ns().env("NS_CONFIG", &cfg)
            .args(["ask", "--role", "review"])
            .write_stdin("x")
            .timeout(Duration::from_secs(3))
            .output()
    })
    .unwrap_err();
    assert!(begun.elapsed() < Duration::from_secs(15));
    let msg = panic.downcast_ref::<String>().unwrap();
    assert!(msg.contains("timed out after 3s"), "{msg}");
    assert!(msg.contains("\"ask\" \"--role\" \"review\""), "{msg}");
    assert!(started.exists(), "the harness never started");
    std::thread::sleep(survivor_check_wait(HUNG_GRANDCHILD_DELAY));
    assert!(!marker.exists(), "the harness's background job outlived ns");
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
    let begun = Instant::now();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.wait())).unwrap_err();
    assert!(begun.elapsed() < Duration::from_secs(15));
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
fn dropping_a_group_kills_its_grandchildren() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("survivor");
    let mut sh = StdCommand::new("sh");
    sh.arg("-c")
        .arg(leaves_a_grandchild(&marker, GRANDCHILD_DELAY))
        .stdout(Stdio::piped());
    let mut group = Group::spawn(&mut sh, common::TIMEOUT);
    let mut line = String::new();
    BufReader::new(group.take_stdout())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line, "started\n");
    drop(group);
    std::thread::sleep(survivor_check_wait(GRANDCHILD_DELAY));
    assert!(!marker.exists(), "the background job outlived the group");
}

const BANNED: [&str; 4] = [
    concat!("assert", "_cmd"),
    concat!("CARGO_BIN", "_EXE"),
    concat!("cargo", "_bin"),
    concat!(".spawn", "()"),
];

fn unbounded_starts(text: &str) -> Vec<(usize, &'static str)> {
    text.lines()
        .enumerate()
        .filter_map(|(n, line)| {
            BANNED
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

#[test]
fn output_returns_when_ns_exits_even_if_its_child_holds_the_pipes() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("survivor");
    let script = format!("(sleep {GRANDCHILD_DELAY}; touch {marker:?}) & echo hi");
    let cfg = harness_config(tmp.path(), &script);
    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review"])
        .write_stdin("x")
        .assert()
        .success()
        .stdout("hi\n");
    std::thread::sleep(survivor_check_wait(GRANDCHILD_DELAY));
    assert!(!marker.exists(), "the harness's background job outlived ns");
}

#[test]
fn output_kills_a_descendant_that_left_the_process_group_and_outlived_its_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("survivor");
    let script = format!("setsid sh -c \"sleep {GRANDCHILD_DELAY}; touch {marker:?}\" & echo hi");
    let cfg = harness_config(tmp.path(), &script);
    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review"])
        .write_stdin("x")
        .assert()
        .success()
        .stdout("hi\n");
    std::thread::sleep(survivor_check_wait(GRANDCHILD_DELAY));
    assert!(!marker.exists(), "the detached job outlived ns");
}
