//! Only the running phase's own `TMPDIR` is exempt, and a link cannot lead out of it.

use super::*;

const OWN: &str = "/r/.git/ns/units/u1/tmp/build-1";

fn denied_in(cmd: &str, tmpdir: &str) -> bool {
    check_in(&bash_call(cmd), Some(tmpdir)).is_some()
}

fn write_denied_in(path: &str, tmpdir: Option<&str>) -> bool {
    let call = json!({"tool_name": "Write", "tool_input": {"file_path": path}});
    check_in(&call, tmpdir).is_some()
}

#[test]
fn a_cd_after_a_cd_into_tmpdir_does_not_let_a_link_out() {
    for c in [
        "cd $TMPDIR && cd . && ln -s ../.. up",
        "cd $TMPDIR; cd \"$PWD\"; ln -s ../.. up",
        "cd $TMPDIR && cd sub && cd - && ln -s ../.. up",
        "cd $TMPDIR && cd /repo && ln -s ../.. up",
        "cd $TMPDIR && cd /repo && link a up",
        "cd $TMPDIR && cd -P . && cp -rs ../.. up",
        "pushd $TMPDIR; pushd /repo; popd; ln -s x up",
        "cd $TMPDIR && cd . && echo {} > ../../phase-settings.json",
        "cd $TMPDIR; cd \"$PWD\"; echo {} > ../../x",
        "cd $TMPDIR; cd sub; cd /repo; cd -; echo {} > ../../x",
        "pushd $TMPDIR; pushd /repo; popd; echo {} > ../../x",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_write_through_a_symlink_out_of_the_phase_tmpdir_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let unit = dir.path().join(".git/ns/units/u1");
    let tmp = unit.join("tmp/build-1");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(unit.join("phase-settings.json"), "{}").unwrap();
    std::os::unix::fs::symlink("../..", tmp.join("up")).unwrap();
    let tmp = tmp.to_str().unwrap();
    for p in [
        format!("{tmp}/up/phase-settings.json"),
        format!("{tmp}/up/new-file"),
        format!("{tmp}/up/missing/dir/file"),
    ] {
        assert!(write_denied_in(&p, Some(tmp)), "{p}");
    }
    for p in [
        format!("{tmp}/notes.txt"),
        format!("{tmp}/sub/dir/notes.txt"),
    ] {
        assert!(!write_denied_in(&p, Some(tmp)), "{p}");
    }
}

#[test]
fn a_relative_write_is_resolved_from_the_calls_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let unit = dir.path().join(".git/ns/units/u1");
    let tmp = unit.join("tmp/build-1");
    std::fs::create_dir_all(&tmp).unwrap();
    std::os::unix::fs::symlink("../..", tmp.join("up")).unwrap();
    let tmp = tmp.to_str().unwrap();
    let call = json!({
        "tool_name": "Write",
        "cwd": tmp,
        "tool_input": {"file_path": "up/phase-settings.json"},
    });
    assert!(check_in(&call, Some(tmp)).is_some());
}

#[test]
fn only_the_phases_own_tmpdir_is_exempt() {
    for p in [OWN, "/r/.git/ns/units/u1/tmp/build-1/notes.txt"] {
        assert!(!write_denied_in(p, Some(OWN)), "{p}");
    }
    for p in [
        "/r/.git/ns/units/other/tmp/build-1/x",
        "/r/.git/ns/units/u1/tmp/build-2/x",
        "/r/.git/ns/units/u1/tmp/build-10/x",
        "/r/.git/ns/units/u1/tmp/x",
        "/r/.git/ns/units/u1/tmp",
        "/other/.git/ns/units/u1/tmp/build-1/x",
        ".git/ns/units/u1/tmp/build-1/x",
    ] {
        assert!(write_denied_in(p, Some(OWN)), "{p}");
    }
    assert!(denied_in("rm -rf /r/.git/ns/units/other/tmp/build-1", OWN));
    assert!(denied_in(
        "echo x > /r/.git/ns/units/other/tmp/build-1/x",
        OWN
    ));
    assert!(!denied_in(
        "echo x > /r/.git/ns/units/u1/tmp/build-1/x",
        OWN
    ));
}

#[test]
fn no_tmpdir_is_exempt_without_a_phase_tmpdir_of_the_right_shape() {
    let p = "/r/.git/ns/units/u1/tmp/build-1/x";
    for tmpdir in [
        None,
        Some("/tmp"),
        Some(""),
        Some("r/.git/ns/units/u1/tmp/build-1"),
        Some("/r/.git/ns/units/u1/tmp"),
        Some("/r/.git/ns/units/u1/tmp/build"),
        Some("/r/.git/ns/units/u1/tmp/build-x"),
        Some("/r/.git/ns/units/u1/tmp/build-1/../.."),
        Some("/r/.git/ns/units/*/tmp/build-1"),
        Some("/r/.git/ns/units/u1/other/build-1"),
    ] {
        assert!(write_denied_in(p, tmpdir), "{tmpdir:?}");
    }
}

#[test]
fn a_file_named_like_a_verb_in_tmpdir_is_not_a_verb() {
    for c in [
        "touch $TMPDIR/link",
        "mkdir $TMPDIR/ln",
        "curl -o $TMPDIR/link x",
        "touch \"$TMPDIR/cp\"",
        "mkdir -p $TMPDIR/install",
    ] {
        assert!(!denied(c), "{c}");
    }
    for c in [
        "ln -s ../.. $TMPDIR/x",
        "ln -s ../.. $TMPDIR/link",
        "link a $TMPDIR/ln",
        "touch $TMPDIR/link $TMPDIR/x",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_plain_copy_into_the_phases_own_tmpdir_by_absolute_path_is_allowed() {
    for c in [
        "cp a /r/.git/ns/units/u1/tmp/build-1/a",
        "cp -r src /r/.git/ns/units/u1/tmp/build-1/copy",
        "rsync -a src/ /r/.git/ns/units/u1/tmp/build-1/x",
        "install -m755 foo /r/.git/ns/units/u1/tmp/build-1/bin/foo",
        "cp /r/.git/ns/units/u1/tmp/build-1/a /r/.git/ns/units/u1/tmp/build-1/b",
    ] {
        assert!(!denied_in(c, OWN), "{c}");
        assert!(denied(c), "without a phase TMPDIR: {c}");
    }
    for c in [
        "cp a /r/.git/ns/units/u2/tmp/build-1/a",
        "cp a /r/.git/ns/units/u1/tmp/build-1/../../phase-settings.json",
        "cp a /r/.git/ns/units/u1/tmp/build-1/../x",
        "rsync a /r/.git/ns/units/u1/tmp/b*/x",
        "cp /r/.git/ns/units/u1/phase-settings.json /r/.git/ns/units/u1/tmp/build-1/a",
        "ln -s a /r/.git/ns/units/u1/tmp/build-1/l",
        "cp -s a /r/.git/ns/units/u1/tmp/build-1/l",
    ] {
        assert!(denied_in(c, OWN), "{c}");
    }
}
