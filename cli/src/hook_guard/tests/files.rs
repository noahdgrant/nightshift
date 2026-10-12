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
    for p in ["/r/.claude/commands/x.md", "/r/settings.json"] {
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

#[test]
fn the_units_skill_snapshot_and_its_directory_are_protected() {
    for p in [
        "/r/.ns/u1/skills/ns-x/SKILL.md",
        ".ns/u1/skills",
        "/r/.ns/u1",
        ".ns/u1/",
        ".ns",
        "/r/.ns/u1/sk*",
    ] {
        let call = json!({"tool_name": "Write", "tool_input": {"file_path": p}});
        assert!(check(&call).is_some(), "{p}");
    }
    for c in [
        "rm -rf .ns/u1/skills",
        "sed -i s/a/b/ .ns/u1/skills/ns-x/SKILL.md",
        "rm -rf .ns/u1",
        "rm -rf .ns/u1/",
        "rm -rf .ns",
        "rm -rf .ns/*",
        "rm -rf .ns/*/skills",
        "rm -rf .ns/u1/sk*",
        "rm -rf /r/.ns/u1",
        "mv .ns/u1 /tmp/x",
        "cp x .ns/u1/",
        "cp -r y .ns/u1",
        "ln -s .ns/u1 elsewhere",
        "echo x > .ns/u1",
        "cd .ns/u1 && rm -rf .",
        "cd .ns/u1 && rm -rf ../u1",
        "cd .ns && mv u1 x",
        "git -C .ns/u1 rm -r skills",
        "git -C .ns/u1 mv skills x",
        "git -C .ns rm -r u1",
        "rm -rf .ns/u1/skill?",
        "rm -rf .ns/u1/s[k]ills",
        "rm -rf .ns/u1/ski??s",
        "cd .githooks && git -C ../.ns/u1 rm -r skills",
        "cd .ns/u1 && git -C .. rm u1",
        "git -C /abs/.ns/u1 rm -r skills",
        "cd .githooks && git -C sub rm x",
        "git -C ~/x/.ns/u1 rm -r skills",
        "pushd .ns/u1; rm -rf skills",
    ] {
        assert!(denied(c), "{c}");
    }
    for p in [
        "/r/.ns/u1/build.md",
        ".ns/u1/history/review-1.md",
        "/r/skills/ns-x/SKILL.md",
        "/r/phase-settings.json",
        "/r/.ns/u1/phase-settings.json",
    ] {
        let call = json!({"tool_name": "Write", "tool_input": {"file_path": p}});
        assert!(check(&call).is_none(), "{p}");
    }
    for c in [
        "cp x .ns/u1/build.md",
        "rm .ns/u1/build.md",
        "rm -rf .ns/u1/history",
        "rm .ns/u1/*.md",
        "git -C .ns/u1 status",
        "git -C .ns/u1 rm build.md",
        "cd .ns/u1 && rm build.md",
        "cp .ns/u1/build.md /tmp/x",
        "rm .ns/u1/bui*",
        "rm .ns/u1/sk?",
        "cd .githooks && git -C /tmp rm x",
        "git -C ~/x rm skills",
        "cd .ns/u1; popd; rm -rf skills",
        "cd .ns/u1 && cd ../.. && rm -rf skills",
    ] {
        assert!(!denied(c), "{c}");
    }
}

/// A symlink in a temp dir named like a skill link, to `target`.
fn skills_link(dir: &Path, link: &str, target: &str) -> String {
    let path = dir.join(link);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(target, &path).unwrap();
    path.to_string_lossy().into_owned()
}

fn edits(path: &str) -> bool {
    let call =
        json!({"tool_name": "Edit", "tool_input": {"file_path": format!("{path}/x/SKILL.md")}});
    check(&call).is_some()
}

#[test]
fn a_skills_link_is_protected_only_when_it_points_into_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    for link in [".claude/skills", ".agents/skills"] {
        let dir = dir.path().join(link.replace('/', "_"));
        let into = skills_link(&dir, link, "../.ns/u1/skills");
        assert!(edits(&into), "{link} into a snapshot");
        let elsewhere = skills_link(&dir.join("e"), link, "../tracked");
        assert!(!edits(&elsewhere), "{link} to a tracked dir is editable");
        let real = dir.join("r").join(link);
        std::fs::create_dir_all(&real).unwrap();
        assert!(
            !edits(real.to_str().unwrap()),
            "{link} real dir is editable"
        );
        let missing = dir.join("m").join(link);
        assert!(!edits(missing.to_str().unwrap()), "{link} missing");
    }
}

#[test]
fn a_skills_link_is_denied_for_bash_by_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    for link in [".claude/skills", ".agents/skills"] {
        let path = skills_link(
            &dir.path().join(link.replace('/', "_")),
            link,
            "../.ns/u1/skills",
        );
        assert!(denied(&format!("rm -rf {path}")), "{link}");
        assert!(
            denied(&format!("sed -i s/a/b/ {path}/ns-x/SKILL.md")),
            "{link}"
        );
    }
}

#[test]
fn cd_into_a_unit_directory_protects_its_relative_skills() {
    for c in [
        "cd .ns/u1 && rm -rf skills",
        "cd .ns/u1; sed -i s/a/b/ skills/ns-x/SKILL.md",
        "cd .ns && rm -rf u1/skills",
        "cd .ns; cd u1; rm -rf skills",
        "cd .ns/u1/skills && rm -rf x",
        "cd .ns/u1/other && rm ../skills/x",
        "cd /r/.ns/u1 && rm -rf skills",
        "(cd .ns/u1 && rm -rf skills)",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "cd .ns/u1 && rm build.md",
        "cd .ns/u1; cd /tmp; rm -rf skills",
        "(cd .ns/u1); rm -rf skills",
    ] {
        assert!(!denied(c), "{c}");
    }
    let call = json!({"tool_name": "Bash", "cwd": "/r/.ns/u1", "tool_input": {"command": "rm -rf skills"}});
    assert!(check(&call).is_some());
}

#[test]
fn the_phase_settings_file_and_its_directories_are_protected() {
    for p in [
        "/r/.git/ns/units/u1/phase-settings.json",
        "/r/.git/ns/units/u1",
        "/r/.git/ns/units",
        "/r/.git/ns/units/u1/other",
        "/r/common/ns/units/u1/phase-settings.json",
    ] {
        let call = json!({"tool_name": "Write", "tool_input": {"file_path": p}});
        assert!(check(&call).is_some(), "{p}");
    }
    for c in [
        "rm /r/.git/ns/units/u1/phase-settings.json",
        "echo {} > /r/.git/ns/units/u1/phase-settings.json",
        "sed -i s/a/b/ /c/ns/units/u1/phase-settings.json",
        "rm -rf /r/.git/ns/units",
        "echo {} > $NS_PHASE_SETTINGS",
        "rm \"${NS_PHASE_SETTINGS}\"",
    ] {
        assert!(denied(c), "{c}");
    }
    for p in [
        "/r/phase-settings.json",
        "/r/ns/units/u1/other.json",
        "/r/ns/units",
        "/r/.ns/u1/phase-settings.json",
    ] {
        let call = json!({"tool_name": "Write", "tool_input": {"file_path": p}});
        assert!(check(&call).is_none(), "{p}");
    }
}

#[test]
fn the_ns_directory_under_the_git_dir_cannot_be_swapped() {
    for c in [
        "mv /r/.git/ns /tmp/x",
        "rm -rf /r/.git/ns",
        "mv /tmp/y /r/.git/ns",
        "cp -r /tmp/y/. /r/.git/ns/",
        "cp -r /tmp/y/. /r/.git/ns/units/",
        "ln -sfn /tmp/y /r/.git/ns",
        "rm -rf /r/.git/ns/units",
        "mv /r/.git/ns/units /tmp/x",
        "mv /tmp/y /r/.git/ns/units/u1",
        "rm -rf /r/.git/n*",
        "rm -rf /r/.git/*",
        "rm -rf /r/.git/[n]s",
        "cd /r && mv .git/ns /tmp/x",
        "cd /r/.git && mv ns /tmp/x",
        "cd /r/.git && cp -r /tmp/y/. ns/",
        "cd /r/.git/ns && rm -rf units",
        "git -C /r/.git mv ns /tmp/x",
        "mv /r/.git /tmp/x",
        "mv /tmp/y /r/.git",
        "rm -rf /r/.git",
    ] {
        assert!(denied(c), "{c}");
    }
    for p in ["/r/.git/ns", "/r/.git/ns/x", "/r/.git/ns/units", "/r/.git"] {
        let call = json!({"tool_name": "Write", "tool_input": {"file_path": p}});
        assert!(check(&call).is_some(), "{p}");
    }
}

#[test]
fn reading_the_ns_directory_and_other_ns_commands_stay_allowed() {
    for c in [
        "ls /r/.git/ns",
        "cat /r/.git/ns/units/u1/phase-settings.json",
        "cp /r/.git/ns/units/u1/phase-settings.json /tmp/copy",
        "cp -r /r/.git/ns /tmp/copy",
        "ns ask --harness claude hello",
        "ns verify",
        "mv /tmp/y /tmp/x",
        "rm -rf /r/ns",
        "rm -rf /r/.git/ns-other",
        "rm -rf /r/.github",
        "rm -rf /r/foo.git",
    ] {
        assert!(!denied(c), "{c}");
    }
    for p in ["/r/ns", "/r/foo.git/ns", "/r/.github/ns"] {
        let call = json!({"tool_name": "Write", "tool_input": {"file_path": p}});
        assert!(check(&call).is_none(), "{p}");
    }
}

#[test]
fn glob_matches_reads_star_question_mark_and_sets() {
    for (pattern, text, want) in [
        ("skills", "skills", true),
        ("skills", "skill", false),
        ("*", "skills", true),
        ("*", "", true),
        ("sk*", "skills", true),
        ("*s", "skills", true),
        ("*x", "skills", false),
        ("?", "", false),
        ("?", "a", true),
        ("skill?", "skills", true),
        ("ski??s", "skills", true),
        ("sk?", "skills", false),
        ("s[k]ills", "skills", true),
        ("[", "", true),
        ("", "", true),
        ("", "a", false),
    ] {
        assert_eq!(
            files::glob_matches(pattern.as_bytes(), text.as_bytes()),
            want,
            "{pattern} vs {text}"
        );
    }
}

#[test]
fn a_path_part_that_is_one_expansion_is_one_unknown_directory() {
    for c in [
        "cat > .ns/$U/evidence.md <<'EOF'\nx\nEOF",
        "echo x > \".ns/$U/review.md\"",
        "echo x > .ns/${U}/review.md",
        "echo x > .ns/\"$U\"/review.md",
        "echo x > \".ns/${NS_UNIT}/pr.md\"",
        "cp x .ns/$U/build.md",
        "rm .ns/$U/history/review-1.md",
        "rm .ns/$U/*.md",
        "echo x > $WT/.ns/$U/pr.md",
        "cd .ns/$U && rm build.md",
        "cd .ns && echo x > $U/evidence.md",
        "git -C .ns/$U rm build.md",
        "git rm .ns/$U/build.md",
        "find .ns/$U/history -delete",
        "dd if=x of=.ns/$U/build.md",
        "echo x > \"$F\"",
        "rm \"$(mktemp)\"",
        "echo x > \"$(git rev-parse --show-toplevel)/.ns/u1/build.md\"",
    ] {
        assert!(!denied(c), "{c}");
    }
    for c in [
        "rm -r .ns/$U",
        "rm -rf .ns/$U/",
        "rm -rf \".ns/$U\"",
        "rm -rf .ns/${U}",
        "mv .ns/$U /tmp/x",
        "echo x > .ns/$U",
        "rm -rf .ns/$U/skills",
        "sed -i s/a/b/ .ns/$U/skills/ns-x/SKILL.md",
        "rm -rf .ns/$U/$F",
        "rm -rf .ns/$U/*",
        "ln -sfn .ns/$U/skills .claude/skills",
        "cd .ns/$U && rm -rf skills",
        "cd .ns/$U; rm -rf $F",
        "git -C .ns/$U rm -r skills",
        "cd .ns && rm -rf $U",
        "cd .ns && rm -rf $U/skills",
        "rm -rf .git/$X",
        "rm .githooks/$F",
        "cd .githooks && rm $F",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_path_part_of_unknown_depth_under_ns_is_denied() {
    for c in [
        "echo x > .ns/$A$B/evidence.md",
        "echo x > .ns/\"$A$B\"/evidence.md",
        "echo x > .ns/$U-x/evidence.md",
        "echo x > .ns/$U*/evidence.md",
        "echo x > .ns/{$U,x}/evidence.md",
        "rm -rf .ns/$U/{a,skills}",
        "rm -rf .ns/$U/../u1",
        "rm -rf .ns/$U/..",
        "rm -rf .ns/../$U",
        "rm -rf .ns/$U..",
        "echo x > .ns/u1/../$U/evidence.md",
        "cd .ns/$(f) && rm -rf u1",
        "echo x > .ns/$(cat u)/evidence.md",
        "echo x > .ns/`cat u`/evidence.md",
        "echo x > .ns/${U:-u1}/evidence.md",
        "echo x > .ns/$@/evidence.md",
        "echo x > .ns/$1/evidence.md",
        "echo x > .ns/x$U/evidence.md",
        "cd .ns/u1 && echo x > x/$(f)",
        "rm .githooks/$(f)",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "echo x > /tmp/$A$B",
        "echo x > out/$U-x.md",
        "echo x > $(f)/.ns/u1/build.md",
        "cd .ns/u1 && echo x > /tmp/$(f)",
        "cd .ns/u1 && echo x > \"$(f)/x\"",
        "cd .ns/u1 && rm -f$(x) build.md",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn a_skills_link_is_still_read_past_an_expansion_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = skills_link(dir.path(), ".claude/skills", "../.ns/u1/skills");
    assert!(denied(&format!("rm -rf {path}/$S")));
    assert!(denied(&format!("sed -i s/a/b/ {path}/$S/SKILL.md")));
}
