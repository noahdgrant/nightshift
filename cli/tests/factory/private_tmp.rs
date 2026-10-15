/// Whether bubblewrap can give a process a private `/tmp` on this machine.
fn bwrap_works() -> bool {
    StdCommand::new("bwrap")
        .args(["--dev-bind", "/", "/", "--", "true"])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `ns`'s PATH for `e` with no `bwrap` on it: each directory holding one is replaced by a
/// directory of links to everything else in it.
fn path_without_bwrap(e: &Env) -> String {
    let inherited = std::env::var("PATH").unwrap_or_default();
    let mut dirs = vec![e.bin.clone()];
    dirs.extend(std::env::split_paths(&inherited));
    let mut out = Vec::new();
    for (i, dir) in dirs.into_iter().enumerate() {
        if !dir.join("bwrap").exists() {
            out.push(dir);
            continue;
        }
        let farm = e.base.join(format!("nobwrap-{i}"));
        fs::create_dir_all(&farm).unwrap();
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            if entry.file_name() != "bwrap" {
                let _ = std::os::unix::fs::symlink(entry.path(), farm.join(entry.file_name()));
            }
        }
        out.push(farm);
    }
    std::env::join_paths(out)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// Each build phase writes `/tmp/note`, after recording what it saw: a note already there, the
/// host's `probe`, where `/tmp/note` lands and where `mktemp` makes its file.
fn record_private_tmp(e: &Env, probe: &Path) {
    e.ctl(
        "build.sh",
        &format!(
            r#"out="$FAKE_CTRL/saw-$NS_UNIT"
if [ -e /tmp/note ]; then echo "note $(cat /tmp/note)" >> "$out"; else echo "no note" >> "$out"; fi
if [ -e {probe:?} ]; then echo "host file" >> "$out"; else echo "no host file" >> "$out"; fi
echo "$NS_UNIT" > /tmp/note
if [ -f "$TMPDIR/note" ]; then echo "note in TMPDIR" >> "$out"; else echo "note not in TMPDIR" >> "$out"; fi
f=$(mktemp)
case "$f" in "$TMPDIR"/*) echo "mktemp in TMPDIR" >> "$out";; *) echo "mktemp at $f" >> "$out";; esac
"#
        ),
    );
}

fn saw(e: &Env, unit: &str) -> Vec<String> {
    fs::read_to_string(e.ctrl.join(format!("saw-{unit}")))
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

fn not_isolated_events(e: &Env) -> Vec<Value> {
    run_events(e)
        .into_iter()
        .filter(|ev| ev["event"] == "tmp_not_isolated")
        .collect()
}

#[test]
fn a_phase_writes_tmp_into_its_own_tmpdir_and_sees_no_other_tmp() {
    if !bwrap_works() {
        eprintln!("skipped: bwrap is missing or cannot start here, so phases share /tmp");
        return;
    }
    let e = Env::new();
    e.issue(8, "Fix other", "OPEN");
    let probe = tempfile::Builder::new()
        .prefix("ns-host-probe-")
        .tempfile_in("/tmp")
        .unwrap();
    record_private_tmp(&e, probe.path());
    e.queue(&format!("build.{UNIT}"), &["fail:script", "pass:script"]);
    e.queue("build.8-fix-other", &["pass:script"]);
    for n in ["7", "8"] {
        let v = e.run(&["run", "--issue", n], 0);
        assert_eq!(v["outcome"], "done", "{v}");
    }
    let first = [
        "no note",
        "no host file",
        "note in TMPDIR",
        "mktemp in TMPDIR",
    ];
    assert_eq!(saw(&e, UNIT), [first, first].concat());
    assert_eq!(saw(&e, "8-fix-other"), first);
    let kept = unit_tmp(&e, UNIT).join("build-1/note");
    assert_eq!(fs::read_to_string(kept).unwrap(), format!("{UNIT}\n"));
    assert!(not_isolated_events(&e).is_empty());
}

#[test]
fn without_bwrap_a_phase_runs_as_before_and_the_run_logs_it_once() {
    let e = Env::new();
    record_tmpdir(&e);
    e.queue("build", &["fail:script", "pass:script"]);
    let out = e
        .ns()
        .env("PATH", path_without_bwrap(&e))
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "done", "{v}");
    let tmp = unit_tmp(&e, UNIT);
    assert_eq!(
        phase_tmpdirs(&e, UNIT),
        ["build-1", "build-2"].map(|d| tmp.join(d).to_str().unwrap().to_string())
    );
    let evs = not_isolated_events(&e);
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert_eq!(evs[0]["reason"], "bwrap is not on PATH", "{}", evs[0]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("phases share the host /tmp: bwrap is not on PATH"),
        "{stderr}"
    );
}

#[test]
fn a_bwrap_that_cannot_start_leaves_phases_unwrapped() {
    let e = Env::new();
    write_exe(
        &e.bin.join("bwrap"),
        "#!/bin/sh\necho 'bwrap: setting up uid map: Permission denied' >&2\nexit 1\n",
    );
    record_tmpdir(&e);
    e.queue("build", &["pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let dir = unit_tmp(&e, UNIT).join("build-1");
    assert_eq!(phase_tmpdirs(&e, UNIT), [dir.to_str().unwrap()]);
    let evs = not_isolated_events(&e);
    assert_eq!(evs.len(), 1, "{evs:?}");
    let reason = evs[0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("bwrap failed"), "{reason}");
}

#[test]
fn a_dry_run_checks_no_private_tmp() {
    let e = Env::new();
    write_exe(&e.bin.join("bwrap"), "#!/bin/sh\nexit 1\n");
    let out = e
        .ns()
        .args(["run", "--issue", "7", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("/tmp"), "{stderr}");
}

#[test]
fn a_missing_harness_under_bwrap_is_still_a_usage_error() {
    if !bwrap_works() {
        eprintln!("skipped: bwrap is missing or cannot start here, so phases run unwrapped");
        return;
    }
    let e = Env::new();
    e.factory("[defaults]\nharness = \"gone\"\n");
    e.config("[harness.gone]\ncommand = [\"ns-no-such-harness\"]\ncommand_write = [\"ns-no-such-harness\"]\n");
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(2);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        err.contains("harness binary `ns-no-such-harness` is not on PATH"),
        "{err}"
    );
}
