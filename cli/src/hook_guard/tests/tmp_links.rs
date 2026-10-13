//! A write through a symlink in the phase's `TMPDIR` is denied, however the link got there; a
//! symlink elsewhere in it is not in the way.

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
    ] {
        let why = unit.bash(c).unwrap_or_else(|| panic!("not denied: {c}"));
        assert!(why.contains("symlink") && why.contains("hk"), "{c}: {why}");
    }
    let write = |name: &str| {
        let path = format!("{}/{name}", unit.tmp());
        let call = json!({"tool_name": "Write", "tool_input": {"file_path": path}});
        check_in(&call, Some(unit.tmp()))
    };
    assert!(write("hk/pre-commit").is_some());
    assert_eq!(write("notes.txt"), None);
}

#[test]
fn a_symlink_elsewhere_in_tmpdir_does_not_deny_writes_that_miss_it() {
    let unit = Unit::new();
    std::os::unix::fs::symlink("../../../../../hooks", unit.tmp.join("venv")).unwrap();
    for c in [
        "echo x > $TMPDIR/a",
        "echo x | tee -a $TMPDIR/sub/file",
        "cargo test > \"${TMPDIR}/out.log\"",
        "touch $TMPDIR/new",
    ] {
        assert_eq!(unit.bash(c), None, "{c}");
    }
    let why = unit.bash("echo x > $TMPDIR/venv/pre-commit").unwrap();
    assert!(
        why.contains("venv") && why.contains("is a symlink"),
        "{why}"
    );
}

#[test]
fn a_link_cp_a_copied_into_tmpdir_denies_a_write_through_it() {
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
    let c = "echo '{}' > $TMPDIR/copy/up/phase-settings.json";
    assert!(unit.bash(c).is_some(), "{c}");
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
fn a_tmpdir_too_big_to_walk_still_allows_a_write_that_misses_a_link() {
    let unit = Unit::new();
    let many = unit.tmp.join("many");
    std::fs::create_dir(&many).unwrap();
    for i in 0..20_001 {
        std::fs::File::create(many.join(i.to_string())).unwrap();
    }
    assert_eq!(unit.bash("echo x > $TMPDIR/out.txt"), None);
}

#[test]
fn an_unreadable_directory_in_tmpdir_does_not_deny_a_write_beside_it() {
    use std::os::unix::fs::PermissionsExt;
    let unit = Unit::new();
    let locked = unit.tmp.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read_dir(&locked).is_ok();
    let why = unit.bash("echo x > $TMPDIR/out.txt");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    if readable {
        eprintln!("SKIPPED: a mode 000 directory is readable here (running as root?)");
        return;
    }
    assert_eq!(why, None);
}
