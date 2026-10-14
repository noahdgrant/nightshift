//! `scripts/stress.sh`, the documented way to load the CPU while reproducing a flaky test.

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/stress.sh")
}

fn start(seconds: &str, workers: &str) -> common::Group {
    common::command(script())
        .args([seconds, workers])
        .start_piped()
}

/// The worker pids the script announces on stderr.
fn workers(g: &mut common::Group) -> Vec<String> {
    let mut line = String::new();
    BufReader::new(g.take_stderr())
        .read_line(&mut line)
        .unwrap();
    line.trim()
        .strip_prefix("stress: workers ")
        .unwrap_or_else(|| panic!("no worker line: {line:?}"))
        .split(' ')
        .map(str::to_string)
        .collect()
}

/// Whether `pid` is a live process whose parent is `ppid`.
fn running_under(pid: &str, ppid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    fields[0] != "Z" && fields[1] == ppid.to_string()
}

fn signal(g: &common::Group, sig: libc::c_int) {
    // SAFETY: kill(2) on the one pid this test started.
    assert_eq!(unsafe { libc::kill(g.id() as libc::pid_t, sig) }, 0);
}

/// Whether `pid` is a process that has not exited.
fn alive(pid: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|s| s.rsplit_once(')').unwrap().1.split_whitespace().next() != Some("Z"))
}

#[test]
fn it_runs_its_workers_for_the_given_seconds_and_reaps_them_before_it_exits() {
    let bin = tempfile::tempdir().unwrap();
    let record = bin.path().join("record");
    common::write_exe(
        &bin.path().join("sleep"),
        format!(
            "#!/bin/sh
echo \"$*\" > '{}'\n",
            record.display()
        ),
    );
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut g = common::command(script())
        .args(["7", "3"])
        .env("PATH", path)
        .start_piped();
    let pids = workers(&mut g);
    assert_eq!(pids.len(), 3, "{pids:?}");
    assert!(g.wait().success());
    assert_eq!(std::fs::read_to_string(&record).unwrap(), "7\n");
    for p in &pids {
        assert!(!alive(p), "worker {p} outlived the script");
    }
}

#[test]
fn an_interrupt_stops_every_worker_it_started_and_no_other() {
    for (sig, code) in [(libc::SIGINT, 130), (libc::SIGTERM, 143)] {
        let mut bystander = start("600", "1");
        let other = workers(&mut bystander);
        let mut g = start("600", "4");
        let pids = workers(&mut g);
        assert_eq!(pids.len(), 4, "{pids:?}");
        for p in &pids {
            assert!(running_under(p, g.id()), "worker {p} is not running");
        }
        signal(&g, sig);
        assert_eq!(g.wait().code(), Some(code), "signal {sig}");
        for p in &pids {
            assert!(common::exits(p), "worker {p} outlived signal {sig}");
        }
        assert!(
            running_under(&other[0], bystander.id()),
            "signal {sig} to one script stopped another's worker"
        );
    }
}

#[test]
fn bad_or_missing_arguments_exit_2_with_usage_and_start_nothing() {
    for args in [
        &[][..],
        &["10"],
        &["10", "2", "3"],
        &["0", "2"],
        &["10", "0"],
        &["-1", "2"],
        &["10", "x"],
        &["1.5", "2"],
        &["", "2"],
        &["10", "02x"],
    ] {
        let out = common::command(script()).args(args).output();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {err}");
        assert_eq!(
            err, "usage: scripts/stress.sh <seconds> <workers> (both positive integers)\n",
            "{args:?}"
        );
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn a_worker_stops_itself_when_the_script_is_killed_outright() {
    let mut g = start("1", "2");
    let pids = workers(&mut g);
    signal(&g, libc::SIGKILL);
    g.wait();
    for p in &pids {
        assert!(common::exits(p), "worker {p} outlived its own deadline");
    }
}
