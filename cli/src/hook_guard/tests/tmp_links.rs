//! A symlink anywhere in the phase's `TMPDIR` turns off its exemption, however it got there.

use super::*;

struct Unit {
    _dir: tempfile::TempDir,
    tmp: std::path::PathBuf,
}

impl Unit {
    fn new() -> Unit {
        let dir = tempfile::tempdir().unwrap();
        let git = dir.path().canonicalize().unwrap().join(".git");
        let tmp = git.join("ns/units/u1/tmp/build-1");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::create_dir_all(git.join("hooks")).unwrap();
        std::fs::write(git.join("ns/units/u1/phase-settings.json"), "{}").unwrap();
        Unit { _dir: dir, tmp }
    }

    fn tmp(&self) -> &str {
        self.tmp.to_str().unwrap()
    }

    fn bash(&self, cmd: &str) -> Option<String> {
        check_in(&bash_call(cmd), Some(self.tmp()))
    }
}

#[test]
fn a_git_alias_that_runs_ln_is_denied() {
    for c in [
        "git -c 'alias.l=!ln -s ../../../../../hooks $TMPDIR/hk' l",
        "git -c alias.l='!ln -s ../.. $TMPDIR/up' l",
        "git --config-env alias.l=V l link $TMPDIR/b",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git -c alias.st=status st",
        "git log --grep link > $TMPDIR/log.txt",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn a_write_through_a_link_a_git_alias_planted_is_denied_and_names_the_link() {
    let unit = Unit::new();
    std::os::unix::fs::symlink("../../../../../hooks", unit.tmp.join("hk")).unwrap();
    for c in [
        "echo x > $TMPDIR/hk/pre-commit",
        "echo x | tee -a $TMPDIR/hk/pre-commit",
        "echo x > \"${TMPDIR}/hk/pre-commit\"",
        "echo x > $TMPDIR/out.txt",
    ] {
        let why = unit.bash(c).unwrap_or_else(|| panic!("not denied: {c}"));
        assert!(why.contains("symlink") && why.contains("hk"), "{c}: {why}");
    }
    let write = json!({
        "tool_name": "Write",
        "tool_input": {"file_path": format!("{}/notes.txt", unit.tmp())},
    });
    assert!(check_in(&write, Some(unit.tmp())).is_some());
}

#[test]
fn a_link_cp_a_copied_into_tmpdir_turns_off_the_exemption() {
    let unit = Unit::new();
    let src = unit.tmp.parent().unwrap().parent().unwrap().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::os::unix::fs::symlink("../..", src.join("up")).unwrap();
    let copied = std::process::Command::new("cp")
        .arg("-a")
        .arg(&src)
        .arg(unit.tmp.join("copy"))
        .status()
        .unwrap();
    assert!(copied.success());
    for c in [
        "echo '{}' > $TMPDIR/copy/up/phase-settings.json",
        "echo x > /dev/null; touch $TMPDIR/new",
    ] {
        assert!(unit.bash(c).is_some(), "{c}");
    }
    let abs = format!("echo x > {}/copy/up/phase-settings.json", unit.tmp());
    assert!(unit.bash(&abs).is_some(), "{abs}");
}

#[test]
fn a_tmpdir_with_no_symlink_still_allows_writes() {
    let unit = Unit::new();
    std::fs::create_dir_all(unit.tmp.join("sub/deeper")).unwrap();
    std::fs::write(unit.tmp.join("sub/file"), "x").unwrap();
    for c in [
        "echo x > $TMPDIR/out.txt".to_string(),
        "echo x | tee -a $TMPDIR/sub/file".to_string(),
        "cp -r src $TMPDIR/copy".to_string(),
        format!("echo x > {}/sub/deeper/n.txt", unit.tmp()),
        "cargo test > \"$TMPDIR/out.log\"".to_string(),
    ] {
        assert_eq!(unit.bash(&c), None, "{c}");
    }
    assert!(unit
        .bash("echo {} > $TMPDIR/../../phase-settings.json")
        .is_some());
}

#[test]
fn a_tmpdir_too_big_to_walk_exempts_nothing() {
    let unit = Unit::new();
    let many = unit.tmp.join("many");
    std::fs::create_dir(&many).unwrap();
    for i in 0..20_001 {
        std::fs::File::create(many.join(i.to_string())).unwrap();
    }
    let why = unit.bash("echo x > $TMPDIR/out.txt").unwrap();
    assert!(why.contains("TMPDIR"), "{why}");
}

#[test]
fn a_tmpdir_that_cannot_be_read_exempts_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let unit = Unit::new();
    let locked = unit.tmp.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read_dir(&locked).is_ok();
    let why = unit.bash("echo x > $TMPDIR/out.txt");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !readable {
        assert!(why.is_some());
    }
}
