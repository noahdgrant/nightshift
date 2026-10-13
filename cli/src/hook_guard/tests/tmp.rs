//! The unit tmp directory, link makers and unknown-depth expansions.

use super::files::skills_link;
use super::*;

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
fn ln_and_link_naming_tmpdir_the_units_directory_or_the_settings_file_are_denied() {
    for c in [
        "ln -s ../../phase-settings.json l",
        "ln -s .. $TMPDIR/l",
        "ln -s /r/.git/ns/units/u1 l",
        "ln -s ns/units l",
        "link /r/.git/ns/units/u1/phase-settings.json l",
        "link a $TMPDIR/l",
        "ln -s a /r/.git/ns/units/u1/tmp/build-1/l",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn ln_with_a_harmless_target_into_tmpdir_is_denied_on_purpose() {
    assert!(denied("ln -s a $TMPDIR/l"));
}

#[test]
fn a_link_maker_is_found_at_any_token_position() {
    for c in [
        "/bin/ln -s a $TMPDIR/l",
        "\\ln -s a $TMPDIR/l",
        "\"ln\" -s a $TMPDIR/l",
        "'link' a $TMPDIR/l",
        "command ln -s a $TMPDIR/l",
        "env ln -s a $TMPDIR/l",
        "FOO=1 ln -s a $TMPDIR/l",
        "echo a | xargs -n 1 ln -s a $TMPDIR/l",
        "busybox ln -s /r/.git/ns/units/u1 $TMPDIR/l",
        "echo $(ln -s a $TMPDIR/l)",
        "(ln -s a $TMPDIR/l)",
        "echo `ln -s a $TMPDIR/l`",
        "true; ln -s a $TMPDIR/l",
        "find . -maxdepth 0 -exec ln -s ../.. $TMPDIR/l \\;",
        "find $TMPDIR -prune -exec ln -s .. {}/l \\;",
        "find $TMPDIR -prune -exec cp -s a {}/l \\;",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn cp_rsync_and_install_naming_the_units_directory_or_the_settings_file_are_denied() {
    for c in [
        "cp /r/.git/ns/units/u1/phase-settings.json /tmp/copy",
        "cp -r /r/.git/ns/units/u1 /tmp/copy",
        "cp x phase-settings.json",
        "rsync -a /r/.git/ns/units/u1/ /tmp/x",
        "rsync x ../../phase-settings.json",
        "install /r/.git/ns/units/u1/phase-settings.json $TMPDIR/l",
        "install x ns/units/u1/y",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn cp_with_a_link_flag_and_tmpdir_is_denied() {
    for c in [
        "cp -s a $TMPDIR/l",
        "cp -l a $TMPDIR/l",
        "cp -rs a $TMPDIR/l",
        "cp -sv a $TMPDIR/l",
        "cp --symbolic-link a $TMPDIR/l",
        "cp --link a $TMPDIR/l",
        "cp $TMPDIR/a -s b",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn copies_that_create_no_link_are_allowed() {
    for c in [
        "install -m755 target/debug/foo $TMPDIR/bin/foo",
        "install -s a $TMPDIR/b",
        "cp -r src $TMPDIR/copy",
        "cp -a src $TMPDIR/copy",
        "cp -v a $TMPDIR/b",
        "cp a $TMPDIR/b",
        "cp ../docs/x.md $TMPDIR/x.md",
        "cp -s a b",
        "cp --symbolic-link a b",
        "rsync -a repo/ $TMPDIR/x",
        "rsync -l a $TMPDIR/x",
        "rsync -v a $TMPDIR/b",
        "rsync -a src/ dst/",
        "cp -r /r/.git/ns /tmp/copy",
        "xargs -s 100 cp a $TMPDIR/b",
        "cp lsx $TMPDIR/x",
        "cp --force --no-clobber a $TMPDIR/b",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn a_link_maker_behind_a_wrapper_a_redirection_or_a_shell_construct_is_denied() {
    for c in [
        "sh -c 'ln -s a $TMPDIR/l'",
        "bash -c 'ln -s a $TMPDIR/l'",
        "timeout 5 ln -s a $TMPDIR/l",
        "stdbuf -o0 ln -s a $TMPDIR/l",
        "ionice -c3 ln -s a $TMPDIR/l",
        "flock /tmp/x ln -s a $TMPDIR/l",
        "strace ln -s a $TMPDIR/l",
        "taskset 1 ln -s a $TMPDIR/l",
        "parallel ln -s a $TMPDIR/l",
        "ssh host ln -s a $TMPDIR/l",
        "su -c \"ln -s a $TMPDIR/l\"",
        "watch ln -s a $TMPDIR/l",
        "unshare -r ln -s a $TMPDIR/l",
        "chroot / ln -s a $TMPDIR/l",
        ">/dev/null ln -s a $TMPDIR/l",
        "2>&1 ln -s a $TMPDIR/l",
        "> /dev/null ln -s a $TMPDIR/l",
        "FOO=\"a b\" ln -s a $TMPDIR/l",
        "case x in x) ln -s a $TMPDIR/l;; esac",
        "f() { ln -s a $TMPDIR/l; }; f",
        "function f { ln -s a $TMPDIR/l; }; f",
        "( ln -s a $TMPDIR/l )",
        "echo \"$( ln -s a $TMPDIR/l)\"",
        "echo $( ln -s a $TMPDIR/l)",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_verb_name_that_is_only_an_argument_is_allowed() {
    for c in [
        "cargo test link > $TMPDIR/out.log",
        "cargo test ln > $TMPDIR/x",
        "grep link f > $TMPDIR/x",
        "git log --grep cp > $TMPDIR/x",
        "echo install $TMPDIR/x",
        "echo link $TMPDIR/x",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn a_link_maker_that_names_nothing_protected_is_allowed() {
    for c in [
        "ln -s a b",
        "ln -s build/runs/units/x l",
        "ln -s build/ns/other/units l",
        "ln -s ns-units l",
        "cp build/runs/units/x y",
        "link a b",
        "ln -s a b; echo x > $TMPDIR/out",
        "cargo install --path . > $TMPDIR/out",
        "echo x > $TMPDIR/out",
        "ln a b | tee $TMPDIR/out",
        "ln a b & echo > $TMPDIR/out",
        "ln a b\necho $TMPDIR/out",
        "echo `ln a b` $TMPDIR/out",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn the_taint_of_a_cd_into_tmpdir_ends_with_its_subshell_or_a_later_cd() {
    for c in [
        "(cd \"$TMPDIR\" && tar xf a.tar); cargo test --manifest-path ../Cargo.toml",
        "(cd $TMPDIR && tar xf a.tar) && cat ../x",
        "cd $TMPDIR && tar xf a.tar; cd /repo && cargo test --manifest-path ../Cargo.toml",
        "cd $TMPDIR; cd; cat ../x",
        "cd $TMPDIR; cd -; cat ../x",
        "cd $TMPDIR; cd; ../x",
        "echo $(cd $TMPDIR && pwd); cat ../x",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn the_taint_of_a_cd_into_tmpdir_survives_a_command_substitution_and_a_cd_option() {
    for c in [
        "cd $TMPDIR && echo $(pwd) && echo {} > ../../phase-settings.json",
        "cd -P $TMPDIR && echo {} > ../../x",
        "cd $TMPDIR; (cd /repo); cat ../x",
        "cd $TMPDIR && echo {} > \"../..\"/x",
        "cd $TMPDIR && cp --target-directory=.. a",
        "cd $TMPDIR && echo {} > \\.\\./\\.\\./x",
        "cd $TMPDIR && echo {} > '..'/'..'/x",
        "echo `cd $TMPDIR`; echo {} > $TMPDIR/\\.\\./\\.\\./x",
        "$(cd $TMPDIR && echo {} > ../../x)",
        "echo ) ; cd $TMPDIR && echo {} > ../../x",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn reading_the_ns_directory_and_other_ns_commands_stay_allowed() {
    for c in [
        "ls /r/.git/ns",
        "cat /r/.git/ns/units/u1/phase-settings.json",
        "cat /r/.git/ns/units/u1/tmp/x",
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
            crate::hook_guard::files::glob_matches(pattern.as_bytes(), text.as_bytes()),
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
        "cd .ns/u1 && rm -rf $(echo ..)/u1/skills",
        "cd .ns/u1 && echo x > \"$(f)/x\"",
        "cd .githooks && echo x > \"$(f)/pre-commit\"",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "echo x > /tmp/$A$B",
        "echo x > out/$U-x.md",
        "echo x > $(f)/.ns/u1/build.md",
        "cd .ns/u1 && echo x > /tmp/$(f)",
        "cd /tmp && echo x > \"$(f)/x\"",
        "cd src && echo x > $(f)/x",
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

#[test]
fn a_cd_of_unknown_depth_under_a_protected_path_is_denied() {
    for c in [
        "cd .ns/$U/.. && rm -rf u1",
        "cd .ns/$U$V && echo x > skills/a",
        "cd .ns/${U:-x} && echo x > skills/a",
        "cd .ns/$(f) && echo x > skills/a",
        "cd .githooks/$U$V && rm ../pre-commit",
        "git -C .githooks/$U$V rm ../pre-commit",
        "cd .git/$U$V && echo x > ../hooks/pre-commit",
        "cd .ns/u1 && cd $(f) && rm -rf skills",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "cd .ns/$U && echo x > evidence.md",
        "cd /tmp/$A$B && echo x > y",
        "cd src/$(f) && echo x > y",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn many_adjacent_expansions_in_a_git_push_word_finish_fast() {
    let many = "$a".repeat(40);
    let word = format!("\"r{many}-x\"");
    let start = std::time::Instant::now();
    let long = denied(&format!("git push {word}"));
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert!(!long);
    assert!(denied(&format!("git push \"--no-v{many}\"")));
    assert!(glob_matches(&b"*".repeat(40), b"x"));
    assert!(!glob_matches(
        &[b"*".repeat(40), b"-x".to_vec()].concat(),
        b"--no-verify"
    ));
    assert!(glob_matches(
        &[b"*".repeat(40), b"-verify".to_vec()].concat(),
        b"--no-verify"
    ));
}

#[test]
fn a_cd_to_a_lone_expansion_from_a_protected_cwd_is_denied() {
    for c in [
        "cd .ns/u1 && cd $U && echo x > skills/a",
        "cd .ns/u1 && cd \"$U\" && rm -rf skills",
        "cd .ns/u1 && git -C $U rm -r skills",
        "git -C .ns/u1 -C $U rm -r skills",
        "pushd .ns/u1 && pushd $U && rm -rf skills",
        "cd .ns/u1 && (cd $U; rm -rf skills)",
        "cd .ns/u1; cd $V; cd $V; echo a > skills/x",
        "cd .ns/u1; cd $V; rm -r .",
        "cd .githooks && cd $U && rm pre-commit",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "cd $U && echo x > y",
        "cd /tmp && cd $U && echo x > y",
        "cd .ns/u1 && cd /tmp/$U && echo x > y",
        "cd .ns/$U && echo x > evidence.md",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn many_expansions_in_a_path_word_finish_fast() {
    let many = "/$a".repeat(8000);
    let start = std::time::Instant::now();
    let hit = denied(&format!("rm .ns{many}"));
    let hit_dots = denied(&format!("rm .ns{}", "/./$a".repeat(8000)));
    assert!(start.elapsed() < std::time::Duration::from_millis(500));
    assert!(hit);
    assert!(hit_dots);
}
