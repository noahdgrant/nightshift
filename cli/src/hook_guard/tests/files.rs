//! Protected paths and the file commands that write them.

use super::*;

#[test]
fn hook_edits_are_denied() {
    for c in [
        "rm .githooks/pre-commit",
        "rm -rf .githooks",
        "mv .githooks/pre-push /tmp/",
        "chmod -x .githooks/pre-commit",
        "echo exit 0 > .githooks/pre-commit",
        "echo exit 0>>.githooks/pre-commit",
        "echo x &> .githooks/pre-commit",
        "echo x 2> .githooks/pre-commit",
        "printf x | tee .githooks/pre-push",
        "sed -i s/ci/true/ .githooks/pre-commit",
        "sed -i.bak s/ci/true/ .githooks/pre-commit",
        "sed --in-place s/a/b/ .githooks/pre-commit",
        "cp /tmp/x .githooks/pre-commit",
        "ln -sf /tmp/x .githooks/pre-commit",
        "git rm .githooks/pre-push",
        "git mv .githooks/a .githooks/b",
        "truncate -s0 ./.githooks/pre-commit",
        "rm .git/./config",
        "rm .git//hooks",
    ] {
        assert!(denied(c), "{c}");
    }
    for (tool, key) in [
        ("Edit", "file_path"),
        ("Write", "file_path"),
        ("MultiEdit", "file_path"),
        ("NotebookEdit", "notebook_path"),
    ] {
        let call = json!({"tool_name": tool, "tool_input": {key: "/r/.githooks/pre-commit"}});
        assert!(check(&call).is_some(), "{tool}");
    }
}

#[test]
fn each_protected_path_is_denied_by_name() {
    for path in [
        ".githooks/pre-commit",
        ".git/hooks/pre-push",
        "/repo/.git/config",
        ".git/config.worktree",
        "/repo/.git/worktrees/u1/config.worktree",
        ".claude/settings.json",
        ".claude/settings.local.json",
        ".ns/u1/skills/ns-build/SKILL.md",
        ".ns/$NS_UNIT/skills",
        "$NS_PHASE_SETTINGS",
        "${NS_PHASE_SETTINGS}",
        "/repo/.git/ns/units/u1/phase-settings.json",
    ] {
        assert!(is_protected(path), "{path}");
        assert!(denied(&format!("echo x > {path}")), "{path}");
        let call = json!({"tool_name": "Write", "tool_input": {"file_path": path}});
        assert!(check(&call).is_some(), "{path}");
    }
    for path in [
        ".ns/u1/build.md",
        ".ns/u1/review/cycles.json",
        "src/githooks.rs",
        ".git-blame-ignore-revs",
        ".claude/agents/x.md",
        "docs/config",
        "skills/ns-build/SKILL.md",
    ] {
        assert!(!is_protected(path), "{path}");
    }
}

#[test]
fn a_command_that_only_reads_a_protected_path_passes() {
    for c in [
        "cat .git/config",
        "cp .githooks/pre-commit /tmp/x",
        "ln -s .githooks/pre-commit /tmp/x",
        "sed s/a/b/ .githooks/pre-commit",
        "sed --expression s/a/b/ .githooks/pre-commit",
        "diff .ns/u1/skills/a .ns/u1/skills/b",
        "wc -l $NS_PHASE_SETTINGS",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn a_skills_link_into_a_snapshot_is_protected_and_a_real_directory_is_not() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    std::os::unix::fs::symlink("../.ns/u1/skills", root.join(".claude/skills")).unwrap();
    std::fs::create_dir_all(root.join(".agents/skills")).unwrap();
    let claude = root.join(".claude/skills/ns-x/SKILL.md");
    let agents = root.join(".agents/skills/ns-x/SKILL.md");
    assert!(is_protected(claude.to_str().unwrap()));
    assert!(!is_protected(agents.to_str().unwrap()));
    std::fs::remove_dir(root.join(".agents/skills")).unwrap();
    std::os::unix::fs::symlink("../skills", root.join(".agents/skills")).unwrap();
    assert!(
        !is_protected(agents.to_str().unwrap()),
        "a link elsewhere is the repo's own"
    );
}
