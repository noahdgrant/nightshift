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

fn write_denied(p: &str) -> bool {
    let call = json!({"tool_name": "Write", "tool_input": {"file_path": p}});
    check(&call).is_some()
}

#[test]
fn a_phase_may_write_inside_its_unit_tmp_directory() {
    for p in [
        "/r/.git/ns/units/u1/tmp/build-1/notes.txt",
        "/r/.git/ns/units/u1/tmp/build-1",
        "/r/.git/ns/units/u1/tmp",
    ] {
        assert!(!write_denied(p), "{p}");
    }
    for c in [
        "echo x > /r/.git/ns/units/u1/tmp/build-1/notes.txt",
        "rm -rf /r/.git/ns/units/u1/tmp/build-1",
        "mv /r/.git/ns/units/u1/tmp/build-1/a /r/.git/ns/units/u1/tmp/build-1/b",
        "echo x > $TMPDIR/out.txt",
        "ln -s a b",
        "cd $TMPDIR && mktemp && echo x > out.txt",
        "cat $TMPDIR/phase-settings.json",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn a_path_that_leaves_or_globs_the_unit_tmp_directory_stays_protected() {
    for c in [
        "echo {} > /r/.git/ns/units/u1/tmp/build-1/../../phase-settings.json",
        "rm -rf /r/.git/ns/units/u1/tmp/..",
        "rm -rf /r/.git/ns/units/*/tmp",
        "rm -rf /r/.git/ns/units/u1/tmp/build-1/*",
        "rm -rf /r/.git/ns/units/u1/t*",
        "mv /r/.git/ns/units/u1/tmp/build-1/x /r/.git/ns/units/u1/phase-settings.json",
        "rm -rf /r/.git/ns/units/u1/other/tmp",
        "rm -rf /r/.git/ns/tmp",
        "rm -rf /r/.git/ns/x/u1/tmp/a",
        "rm -rf /r/.git/ns/foo/ns/units/u1/tmp/a",
        "rm -rf /r/.git/ns/a/.git/x/units/u1/tmp/b",
        "cd /r/.git/ns/units/u1/tmp/build-1 && rm ../../phase-settings.json",
        "cd /r/.git/ns/units/u1/tmp/build-1 && touch x",
        "rm -rf /r/.git/ns/units/u1/tmp/b?ild-1/x",
        "rm -rf /r/.git/ns/units/u1/tmp/[b]uild-1/x",
    ] {
        assert!(denied(c), "{c}");
    }
    for p in [
        "/r/.git/ns/units/u1/tmp/../phase-settings.json",
        "/r/.git/ns/units/u1/tmp/b?ild-1/x",
        "/r/.git/ns/units/u1/tmp/[b]uild-1/x",
    ] {
        assert!(write_denied(p), "{p}");
    }
}

#[test]
fn going_up_from_tmpdir_to_the_phase_settings_is_denied() {
    for c in [
        "echo {} > $TMPDIR/../../phase-settings.json",
        "echo {} > ${TMPDIR}/../../phase-settings.json",
        "rm \"$TMPDIR/../../phase-settings.json\"",
        "echo {} > \"$TMPDIR\"/../../phase-settings.json",
        "echo {} > $(echo $TMPDIR)/../../phase-settings.json",
        "cd $TMPDIR && echo {} > ../../phase-settings.json",
        "cd $TMPDIR; echo {} > ../../phase-settings.json",
        "cd \"$TMPDIR\" && echo {} > ../../phase-settings.json",
        "pushd $TMPDIR; echo {} > ../../phase-settings.json",
        "cd $TMPDIR && echo {} > phase-settings.json",
        "cd $TMPDIR && cd .. && cd .. && echo {} > x",
        "cd $TMPDIR && echo {} > /r/.git/ns/units/u1/phase-settings.json",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_dotdot_that_does_not_follow_tmpdir_is_allowed() {
    for c in [
        "cd .. && cargo test > $TMPDIR/out.log",
        "cp ../docs/x.md $TMPDIR/x.md",
        "git -C .. diff > \"$TMPDIR/d.patch\"",
        "cd .. && gh pr create --body-file $TMPDIR/pr.md",
        "cd ../.. ; echo x > $TMPDIR/a ; cat ../x",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn a_link_maker_naming_tmpdir_or_the_units_directory_is_denied() {
    for c in [
        "ln -s ../../phase-settings.json /r/.git/ns/units/u1/tmp/build-1/l",
        "ln -s ../../phase-settings.json $TMPDIR/l",
        "ln -s .. $TMPDIR/l",
        "ln -s /r/.git/ns/units/u1 /r/.git/ns/units/u1/tmp/build-1/l",
        "ln -s ../../phase-settings.json l",
        "ln -s a /r/.git/ns/units/u1/tmp/build-1/l",
        "ln -s a $TMPDIR/l",
        "/bin/ln -s $(dirname $TMPDIR) $TMPDIR/l",
        "\\ln -s a $TMPDIR/l",
        "command ln -s a $TMPDIR/l",
        "env ln -s a $TMPDIR/l",
        "echo a | xargs ln -s $TMPDIR/l",
        "FOO=1 ln -s a $TMPDIR/l",
        "echo $(ln -s a $TMPDIR/l)",
        "true; link /r/.git/ns/units/u1/phase-settings.json /r/.git/ns/units/u1/tmp/build-1/l",
        "cp -l /r/.git/ns/units/u1/phase-settings.json $TMPDIR/",
        "cp -s /r/.git/ns/units/u1/phase-settings.json $TMPDIR/",
        "cp -rs /r/.git/ns/units/u1/*son $TMPDIR/",
        "cp --symbolic-link /r/.git/ns/units/u1/phase-settings.json $TMPDIR/",
        "cp -a /r/.git/ns/units/u1/phase-settings.json $TMPDIR/",
        "install /r/.git/ns/units/u1/phase-settings.json $TMPDIR/l",
        "rsync -a /r/.git/ns/units/u1/ $TMPDIR/",
        "rsync --links /r/.git/ns/units/u1/phase-settings.json $TMPDIR/",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_link_maker_that_names_neither_tmpdir_nor_the_units_directory_is_allowed() {
    for c in [
        "ln -s a b",
        "cp a b",
        "cp ../docs/x.md $TMPDIR/x.md",
        "cp -v a $TMPDIR/b",
        "rsync -v a $TMPDIR/b",
        "rsync -a src/ dst/",
        "echo link $TMPDIR/x",
        "ln -s a b; echo x > $TMPDIR/out",
        "cargo install --path . > $TMPDIR/out",
    ] {
        assert!(!denied(c), "{c}");
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
