//! Only the running phase's own `TMPDIR` is exempt, and a link cannot lead out of it.

use super::*;
use crate::hook_guard::phase_tmp::{find_link, PhaseTmp};

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
        format!("{tmp}/notes.txt"),
    ] {
        assert!(write_denied_in(&p, Some(tmp)), "{p}");
    }
    std::fs::remove_file(format!("{tmp}/up")).unwrap();
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

#[test]
fn substituted_writes_out_only_the_unquoted_variable() {
    let tmp = PhaseTmp::new(Some(OWN));
    for (command, expected) in [
        ("echo x > $TMPDIR/a", format!("echo x > {OWN}/a")),
        ("echo x > ${TMPDIR}/a", format!("echo x > {OWN}/a")),
        ("echo x > \"$TMPDIR/a\"", format!("echo x > \"{OWN}/a\"")),
        ("echo '$TMPDIR/a'", "echo '$TMPDIR/a'".to_string()),
        ("echo $TMPDIR_X/a", "echo $TMPDIR_X/a".to_string()),
        ("echo $TMPDIR2/a", "echo $TMPDIR2/a".to_string()),
        ("echo \\$TMPDIR/a", "echo \\$TMPDIR/a".to_string()),
        ("echo '$TMPDIR' $TMPDIR", format!("echo '$TMPDIR' {OWN}")),
    ] {
        assert_eq!(tmp.substituted(command), expected, "{command}");
    }
}

#[test]
fn a_tmpdir_a_shell_would_split_is_left_as_the_variable() {
    let tmp = PhaseTmp::new(Some("/r/.git/ns/units/u 1/tmp/build-1"));
    assert_eq!(tmp.substituted("echo x > $TMPDIR/a"), "echo x > $TMPDIR/a");
}

#[test]
fn a_distrusted_tmpdir_denies_a_command_it_cannot_write_out() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir
        .path()
        .canonicalize()
        .unwrap()
        .join(".git/ns/units/u1/tmp/build-1");
    std::fs::create_dir_all(&tmp).unwrap();
    std::os::unix::fs::symlink("../..", tmp.join("up")).unwrap();
    let tmp = tmp.to_str().unwrap();
    let why = check_in(&bash_call("echo x > '$TMPDIR/a'"), Some(tmp)).unwrap();
    assert!(why.contains("cannot write out as a path"), "{why}");
    std::fs::remove_file(format!("{tmp}/up")).unwrap();
    assert_eq!(
        check_in(&bash_call("echo x > '$TMPDIR/a'"), Some(tmp)),
        None
    );
}

#[test]
fn a_tmpdir_reached_through_a_symlinked_parent_is_exempt_by_both_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let real = root.join("real/.git/ns/units/u1/tmp");
    std::fs::create_dir_all(real.join("build-1")).unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
    let by_link = format!("{}/alias/.git/ns/units/u1/tmp/build-1", root.display());
    let by_real = format!("{}/real/.git/ns/units/u1/tmp/build-1", root.display());
    for p in [&by_link, &by_real] {
        assert!(
            !write_denied_in(&format!("{p}/n.txt"), Some(&by_link)),
            "{p}"
        );
        assert!(!denied_in(&format!("echo x > {p}/n.txt"), &by_link), "{p}");
    }
    assert!(write_denied_in(
        &format!("{}/real/.git/ns/units/u1/tmp/build-2/n.txt", root.display()),
        Some(&by_link)
    ));
}

#[test]
fn a_phase_tmpdir_that_is_itself_a_symlink_out_exempts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let tmps = root.join(".git/ns/units/u1/tmp");
    std::fs::create_dir_all(&tmps).unwrap();
    std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
    std::os::unix::fs::symlink(root.join(".git/hooks"), tmps.join("build-1")).unwrap();
    let tmp = tmps.join("build-1");
    let tmp = tmp.to_str().unwrap();
    let why = check_in(&bash_call("echo x > $TMPDIR/pre-commit"), Some(tmp)).unwrap();
    assert!(why.contains("it is a symlink"), "{why}");
    assert!(write_denied_in(&format!("{tmp}/pre-commit"), Some(tmp)));
}

#[test]
fn the_walk_allows_the_cap_and_denies_one_over() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..3 {
        std::fs::File::create(dir.path().join(i.to_string())).unwrap();
    }
    assert_eq!(find_link(dir.path(), 3), None);
    let why = find_link(dir.path(), 2).unwrap();
    assert!(why.contains("more than 2 entries"), "{why}");
}

#[test]
fn the_walk_counts_entries_below_subdirectories() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
    std::fs::File::create(dir.path().join("a/b/f")).unwrap();
    assert_eq!(find_link(dir.path(), 3), None);
    assert!(find_link(dir.path(), 2).is_some());
}

#[test]
fn a_directory_that_cannot_be_read_exempts_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read_dir(&locked).is_ok();
    let why = find_link(dir.path(), 100);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    if readable {
        eprintln!("SKIPPED: a mode 000 directory is readable here (running as root?)");
        return;
    }
    let why = why.expect("an unreadable directory must withdraw the exemption");
    assert!(why.contains("could not be read"), "{why}");
}
