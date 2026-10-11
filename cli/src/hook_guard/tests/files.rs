//! Protected paths, file commands and the working directory.

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
        "printf x | tee .githooks/pre-push",
        "sed -i s/ci/true/ .githooks/pre-commit",
        "cp /tmp/x .githooks/pre-commit",
        "git rm .githooks/pre-push",
        "truncate -s0 ./.githooks/pre-commit",
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
fn cd_into_a_git_dir_protects_relative_hooks_and_config() {
    for c in [
        "cd .git && rm -r hooks",
        "cd .git; echo x > config",
        "cd /repo/.git && sed -i s/a/b/ config",
        "cd .git/ && rm config",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn dot_and_empty_components_do_not_hide_a_protected_path() {
    for c in [
        "rm .git/./config",
        "rm .git/./hooks/pre-commit",
        "rm .git//hooks",
        "rm ./.githooks/./pre-commit",
    ] {
        assert!(denied(c), "{c}");
    }
    for (tool, path) in [("Edit", "/r/.git/./config"), ("Write", "/r/.git//hooks/x")] {
        let call = json!({"tool_name": tool, "tool_input": {"file_path": path}});
        assert!(check(&call).is_some(), "{path}");
    }
}

#[test]
fn hooks_dir_edits_by_cd_cluster_flags_and_find_are_denied() {
    for c in [
        "cd .githooks && rm pre-commit",
        "cd .githooks; echo exit 0 > pre-commit",
        "pushd .githooks; chmod -x pre-commit",
        "cd ./.githooks/ && sed -i s/a/b/ pre-push",
        "sed -Ei s/a/b/ .githooks/pre-commit",
        "sed -ni 1p .githooks/pre-commit",
        "sed --in-place s/a/b/ .githooks/pre-commit",
        "perl -pi -e 1 .githooks/pre-commit",
        "perl -i.bak -pe 1 .githooks/pre-commit",
        "cp -t .githooks /tmp/x",
        "cp --target-directory=.githooks /tmp/x",
        "install -t .githooks /tmp/x",
        "mv -t .githooks /tmp/x",
        "find .githooks -delete",
        "find .githooks -exec rm {} ;",
        "find . -path .githooks -exec sed -i s/a/b/ {} +",
        "git -C .githooks rm pre-commit",
        "cd .githooks && git rm pre-commit",
        "dd if=/dev/null of=.githooks/pre-commit",
        "gawk -i inplace 1 .githooks/pre-commit",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "cd .githooks && cat pre-commit",
        "cd cli && rm x",
        "cd .githooks; cd /tmp; rm x",
        "find . -name '*.rs'",
        "find .githooks -name pre-commit",
        "sed -n 1p .githooks/pre-commit",
        "perl -ne print .githooks/pre-commit",
        "perl -MList::Util=sum -e 1 x",
        "dd if=.githooks/pre-commit of=/tmp/x",
        "git -C repo status",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn every_file_tool_is_guarded() {
    for prog in [
        "rm", "rmdir", "unlink", "shred", "mv", "chmod", "chown", "truncate", "tee", "ln", "touch",
    ] {
        assert!(denied(&format!("{prog} .githooks/pre-commit")), "{prog}");
        assert!(!denied(&format!("{prog} src/x")), "{prog}");
    }
    for prog in ["cp", "install", "rsync"] {
        assert!(denied(&format!("{prog} a .githooks/pre-commit")), "{prog}");
        assert!(
            !denied(&format!("{prog} .githooks/pre-commit /tmp/x")),
            "{prog}"
        );
    }
}

#[test]
fn a_subshell_cd_ends_with_the_subshell() {
    for c in [
        "(cd .githooks && rm pre-commit)",
        "(cd .githooks); cd .githooks; rm x",
        "cd .githooks; (rm x)",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in ["(cd .githooks); rm x", "(cd .githooks && cat x); rm x"] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn git_dir_hooks_and_config_are_protected_like_githooks() {
    for c in [
        "rm .git/hooks/pre-commit",
        "echo x > .git/config",
        "sed -i s/a/b/ .git/config",
        "git rm .git/hooks/pre-commit",
        "cd .git/hooks && rm pre-commit",
        "chmod -x /r/.git/hooks/pre-commit",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "cat .git/config",
        "rm .git/index.lock",
        "ls .git/hooks",
        "git -C .git log",
    ] {
        assert!(!denied(c), "{c}");
    }
    for (tool, path) in [
        ("Edit", "/r/.git/hooks/pre-commit"),
        ("Write", "/r/.git/config"),
    ] {
        let call = json!({"tool_name": tool, "tool_input": {"file_path": path}});
        assert!(check(&call).is_some(), "{path}");
    }
}

#[test]
fn payload_cwd_in_a_protected_directory_protects_relative_paths() {
    for cwd in ["/r/.githooks", "/r/.githooks/sub", "/r/.git"] {
        let call =
            json!({"tool_name": "Bash", "cwd": cwd, "tool_input": {"command": "rm pre-commit"}});
        assert!(check(&call).is_some(), "{cwd}");
    }
    for cwd in ["/r", "/r/cli"] {
        let call =
            json!({"tool_name": "Bash", "cwd": cwd, "tool_input": {"command": "rm pre-commit"}});
        assert!(check(&call).is_none(), "{cwd}");
    }
}

#[test]
fn claude_settings_are_protected_and_hooks_stay_enabled() {
    let s: Value = serde_json::from_str(&settings(Path::new("/x/ns"))).unwrap();
    assert_eq!(s["disableAllHooks"], json!(false));
    for p in [
        "/r/.claude/settings.json",
        ".claude/settings.local.json",
        "/r/.claude/settings.foo.json",
    ] {
        let call = json!({"tool_name": "Edit", "tool_input": {"file_path": p}});
        assert!(check(&call).is_some(), "{p}");
    }
    for c in [
        "rm .claude/settings.json",
        "echo '{}' > .claude/settings.local.json",
        "sed -i s/a/b/ .claude/settings.json",
    ] {
        assert!(denied(c), "{c}");
    }
    for p in ["/r/.claude/skills/x.md", "/r/settings.json"] {
        let call = json!({"tool_name": "Edit", "tool_input": {"file_path": p}});
        assert!(check(&call).is_none(), "{p}");
    }
}

#[test]
fn cd_up_keeps_a_protected_directory_unless_provably_outside() {
    assert!(denied("cd .githooks/sub && cd .. && rm x"));
    assert!(denied("cd .githooks && cd ../.githooks && rm x"));
    assert!(!denied("cd .githooks && cd /tmp && rm x"));
}

#[test]
fn chained_find_exec_is_checked_in_linear_time() {
    let chain = "find . -exec find . ".repeat(40);
    let started = std::time::Instant::now();
    assert!(denied(&format!(r"{chain} -exec git commit -n ;")));
    assert!(!denied(&format!(r"{chain} -exec true ;")));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn directory_tracking_is_restored_and_reset_where_it_should_be() {
    for c in [
        "cd .githooks && cd -P . && rm pre-commit",
        "cd -P .githooks && rm pre-commit",
        "pushd .githooks; rm pre-commit; popd; rm pre-commit",
        "cd .githooks; cd -; rm pre-commit",
        "cd .githooks; cd ..; rm pre-commit",
        "git -C .githooks log; git -C .githooks rm x",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git -C .githooks log; rm x",
        "pushd .githooks; popd; rm x",
        "cd .githooks; cd /abs; rm x",
        "cd .githooks; cd ~; rm x",
        "cd -P /tmp && rm x",
        "cd .githooks && cd /tmp/.. && rm pre-commit",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn each_find_exec_command_ends_at_its_terminator() {
    assert!(denied(r"find . -exec true \; -exec git commit -n \;"));
    assert!(denied(r"find . -exec true {} + -execdir git commit -n \;"));
}

#[test]
fn dot_dot_resolves_against_the_depth_of_a_protected_cwd() {
    for c in [
        "cd .githooks/sub && rm ../pre-commit",
        "cd .githooks/a/b && rm ../../pre-commit",
        "cd .githooks/sub && rm ../../.githooks/pre-commit",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "cd .githooks && rm ../x",
        "cd .githooks/sub && rm ../../x",
        "cd .githooks && rm /tmp/x",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn chmod_and_find_delete_on_hooks_are_denied() {
    for c in [
        "chmod -x .githooks/pre-commit",
        "chmod -R a-x .githooks",
        "find .githooks -delete",
        "find . -path '*githooks*' -delete",
        "find .git/hooks -delete",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "chmod +x build.sh",
        "find . -name '*.tmp' -delete",
        "find .githooks -name x",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn relative_cd_inside_a_protected_directory_keeps_the_tracking() {
    for c in [
        "cd .githooks && cd lib && rm ../pre-commit",
        "cd .githooks; cd lib; rm ../pre-commit",
        "pushd .githooks; pushd lib; rm ../x",
        "cd .git/refs && rm ../config",
        "cd .git; cd refs; rm ../hooks/pre-commit",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in ["cd .githooks && rm ../x", "rm .git/index.lock"] {
        assert!(!denied(c), "{c}");
    }
}
