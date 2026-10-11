use super::*;

fn bash_call(cmd: &str) -> Value {
    json!({"tool_name": "Bash", "tool_input": {"command": cmd}})
}

fn denied(cmd: &str) -> bool {
    check(&bash_call(cmd)).is_some()
}

#[test]
fn commit_without_hooks_is_denied() {
    for c in [
        "git commit --no-verify -m x",
        "git commit -n -m x",
        "git commit -am x -n",
        "git commit -anm x",
        "git commit --no-veri -m x",
        "/usr/bin/git commit -n",
        "cd cli && git commit -n",
        "git add . ; git commit -n",
        "git -C repo commit --no-verify",
        "true | git commit -n",
        "(git commit -n)",
        "echo $(git commit -n)",
        "sudo git commit -n",
        "sudo -E git commit -n",
        "timeout 60 git commit -n",
        "timeout -k 5 60 git commit -n",
        "timeout --foreground -s KILL 60 git commit -n",
        "bash -c 'git commit --no-verify -m x'",
        "sh -lc \"git commit -n\"",
        "eval git commit -n",
        "echo 'x' ; git commit -n",
        "echo \"x\"; git commit -n",
        "echo \"a\\\"b\"; git commit -n",
        "git commit -m a#b -n",
        "{ git commit -n; }",
        "echo `git commit -n`",
        "bash -x -c 'git commit -n'",
        "cat <<< 'x'\ngit commit -n",
        "cat <<-EOF > x\n\tHUSKY=0 git push\n\tEOF\ngit commit -n",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn push_merge_and_rebase_without_hooks_are_denied() {
    for c in [
        "git push --no-verify",
        "git push --no-verify origin ns/x",
        "git merge --no-verify main",
        "git rebase --no-verify origin/main",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn hooks_path_overrides_are_denied() {
    for c in [
        "git -c core.hooksPath=/dev/null commit -m x",
        "git -c core.hookspath= push",
        "git --config-env=core.hooksPath=X commit",
        "git config core.hooksPath /tmp/none",
        "git config --local core.hooksPath x",
        "git config --unset core.hooksPath",
        "git config set core.hooksPath x",
        "git config unset core.hooksPath",
    ] {
        assert!(denied(c), "{c}");
    }
}

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
fn env_vars_that_skip_hooks_are_denied() {
    for c in [
        "HUSKY=0 git commit -m x",
        "SKIP=lint git commit -m x",
        "env HUSKY=0 git push",
        "export HUSKY=0",
        "export A=1 HUSKY=0",
        "GIT_CONFIG_KEY_0=core.hooksPath git commit",
        "GIT_CONFIG_VALUE_0=x git commit",
        "export SKIP=all; git commit -m x",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0=x git commit",
        "GIT_CONFIG_PARAMETERS=\"'core.hooksPath'='x'\" git push",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn the_denial_names_the_rule() {
    let why = check(&bash_call("git commit -n -m x")).unwrap();
    assert!(why.contains("git commit -n"), "{why}");
    assert!(RULE.contains("skills/ns-contract"));
    assert!(RULE.contains("Git hooks"));
}

#[test]
fn ordinary_commands_pass() {
    for c in [
        "git commit -m x",
        "git commit -m 'skip --no-verify here'",
        "git commit -m -n",
        "git commit --message -n",
        "git commit -F msg.txt",
        "git commit -Cn",
        "git commit -m x -- -n",
        "git commit --amend --no-edit",
        "git push origin ns/x",
        "git push -n origin ns/x",
        "git merge -n main",
        "git rebase origin/main",
        "git config core.hooksPath",
        "git config --get core.hooksPath",
        "git -c user.name=x commit -m y",
        "echo 'git commit --no-verify'",
        "echo \"HUSKY=0 rm .githooks/pre-commit\"",
        "grep -rn no-verify .githooks/",
        "cat .githooks/pre-commit",
        "cp .githooks/pre-commit /tmp/x",
        "sed -n 1,5p .githooks/pre-push",
        "ls -la # git commit -n",
        "HUSKY=1 git commit -m x",
        "cargo test",
        "cat > build.md <<'EOF'\ngit commit --no-verify\nrm .githooks/pre-commit\nEOF\ngit status",
        "cat <<-EOF > x\n\tHUSKY=0 git push\n\tEOF",
        "cmd 2>&1 | tail",
        "cp a b < .githooks/pre-commit",
        "0=x rm .githooks/pre-commit",
        "git rebase --no origin/main",
        "git commit -uno -m x",
        "export PATH=/x:$PATH",
        "",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn other_tools_and_unreadable_input_pass() {
    let edit = json!({"tool_name": "Edit", "tool_input": {"file_path": "/r/src/githooks.rs"}});
    assert_eq!(check(&edit), None);
    assert_eq!(
        check(&json!({"tool_name": "Read", "tool_input": {"file_path": "/r/.githooks/x"}})),
        None
    );
    assert_eq!(check(&json!({"tool_name": "Bash"})), None);
    assert_eq!(check(&json!({})), None);
}

#[test]
fn heredoc_after_the_body_is_still_checked() {
    assert!(denied("cat > x <<EOF\nhi\nEOF\ngit commit -n"));
}

#[test]
fn shell_reserved_words_do_not_hide_a_command() {
    for c in [
        "if true; then git commit -n; fi",
        "for f in a; do git commit -n; done",
        "! git commit -n",
        "while true; do git commit --no-verify; done",
        "if x; then :; else git push --no-verify; fi",
        "until false; do HUSKY=0 git commit; done",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn wrappers_with_value_options_still_run_the_command() {
    for c in [
        "nice -n 5 git commit -n",
        "nice --adjustment 5 git commit -n",
        "env -u X git commit -n",
        "env -C /tmp git commit -n",
        "env -S 'git commit -n'",
        "env --split-string='git commit -n'",
        "sudo -u u git commit -n",
        "sudo -u u -g g -- git commit -n",
        "xargs -n 1 git commit -n",
        "xargs -I {} git commit -n",
        "command -p git commit -n",
        "exec -a x git commit -n",
        "nohup git commit -n",
        "time -p git commit -n",
        "time -f %e git commit -n",
        "stdbuf -o L git commit -n",
        "setsid git commit -n",
        "doas -u u git commit -n",
        "timeout -s KILL 5 git commit -n",
        "find . -exec git commit -n ;",
        "export A=1; nice -n 1 env A=2 git commit --no-verify",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "nice -n 5 git commit -m x",
        "env -u X git commit -m x",
        "sudo -u u git push origin x",
        "xargs -n 1 echo -n",
        "nice -n 5 echo -n",
        "time -p cargo test",
        "find . -name x -exec echo -n ;",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn shell_expansion_cannot_hide_a_bypass() {
    for c in [
        "git commit $'--no-verify'",
        "git $'commit' -n",
        "git commit --no-verif{y,}",
        "git commit {-n,}",
        "F=--no-verify; git commit $F",
        "git commit \"$F\"",
        "git commit ${F}",
        "git push $(echo --no-verify)",
        "git commit \"$(git commit -n)\"",
        "echo \"`git commit -n`\"",
        "echo \"$(echo $(git commit -n))\"",
        "git commit -m \"$(date)\" -n",
        "cat <<EOF\n$(git commit -n)\nEOF",
        "cat <<EOF\n`git commit -n`\nEOF",
        "cat <(git commit -n)",
        "tee >(git commit -n)",
        "2>/dev/null git commit -n",
        "2>&1 git commit -n",
        "$G commit -n",
        "F=git; $F commit -n",
        "{git,commit,-n}",
        "git $SUB",
        "eval \"$CMD\"",
        "sh -c \"$CMD\"",
        "git commit -m $'x\\x20y' $'\\x2d\\x6e'",
        "git commit &>/dev/null -n",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git commit -m \"$(cat <<'EOF'\nIt's a (fix) with `ticks` and git commit -n\nEOF\n)\"",
        "git commit -m \"msg $(date) end\"",
        "git commit -m \"$MSG\"",
        "git commit -am \"$MSG\"",
        "git push origin ns/$ID",
        "git commit -m $'a\\nb'",
        "echo \"$HOME\" > out.txt",
        "git commit -m 'it is $F'",
        "echo $((1+2))",
        "cat <<EOF\n$HOME and \\$(git commit -n)\nEOF",
        "echo ${HOME}/x",
        "git commit -F - <<EOF\nmsg\nEOF",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn nesting_past_the_cap_is_denied() {
    let mut c = "git status".to_string();
    for _ in 0..12 {
        c = format!("sh -c {}", shell_quote(&c));
    }
    assert!(denied(&c));
    let mut c = "git status".to_string();
    for _ in 0..3 {
        c = format!("sh -c {}", shell_quote(&c));
    }
    assert!(!denied(&c));
}

#[test]
fn git_global_options_cannot_hide_the_subcommand() {
    for c in [
        "git -ccore.hooksPath=/dev/null commit",
        "git --config-env core.hooksPath=VAR commit",
        "git --config-env=core.hooksPath=VAR commit",
        "git --exec-path commit -n",
        "git --exec-path=/x commit -n",
        "git -C repo -c user.name=x commit -n",
        "git --git-dir x commit -n",
        "git --no-pager commit -n",
        "git -c alias.ci='commit -n' ci",
        "git -c 'alias.ci=!git commit -n' ci",
        "git -c alias.ci=commit -c core.hooksPath=x ci",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git -c alias.ci=commit ci -m x",
        "git --exec-path commit -m x",
        "git -ccolor.ui=never commit -m x",
        "git --config-env user.name=NAME commit -m x",
        "git -C .githooks log",
    ] {
        assert!(!denied(c), "{c}");
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
        "cd .githooks && cd .. && rm x",
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
fn more_git_subcommands_and_config_writes_are_denied() {
    for c in [
        "git pull --no-verify",
        "git am --no-verify x.patch",
        "git cherry-pick --no-verify abc",
        "git revert --no-verify abc",
        "git mv .githooks/pre-commit x",
        "git mv x .githooks/pre-commit",
        "git config --edit core.hooksPath",
        "git config --add core.hooksPath x",
        "git config --replace-all core.hooksPath x",
        "git config -e core.hooksPath",
        "git config --global core.hooksPath x",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git pull origin main",
        "git am x.patch",
        "git revert abc",
        "git mv a b",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn other_shells_and_redirect_forms_are_read() {
    for c in [
        "zsh -c 'git commit -n'",
        "dash -c 'git commit -n'",
        "ksh -c 'git commit -n'",
        "bash -ic 'git commit -n'",
        "bash -o pipefail -c 'git commit -n'",
        "echo x &> .githooks/pre-commit",
        "echo x &>> .githooks/pre-commit",
        "echo x >| .githooks/pre-commit",
        "echo x >& .githooks/pre-commit",
        "echo x 2> .githooks/pre-commit",
        "echo x 1>.githooks/pre-commit",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "bash script.sh git commit -n",
        "echo x >&2",
        "echo x 2>&1",
        "echo x >&-",
        "cmd &> /dev/null",
        "cmd >| out.txt",
        "bash -c",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn commit_value_flags_swallow_a_dash_leading_value() {
    for c in [
        "git commit -F -n",
        "git commit -C -n",
        "git commit --file -n",
        "git commit --reuse-message -n",
        "git commit --author -n",
        "git commit --cleanup -n",
        "git commit -m x --template -n",
        "git commit -t -n",
        "git commit --fixup -n",
        "git commit --trailer -n",
        "git commit --date -n",
        "git commit --squash -n",
        "git commit --reedit-message -n",
        "git commit -S -m x",
        "HUSKY=1 git commit -m x",
        "HUSKY=2 git commit -m x",
    ] {
        assert!(!denied(c), "{c}");
    }
    for c in [
        "git commit -F x -n",
        "git commit --file=x -n",
        "git commit --file x --no-verify",
        "git commit -sn",
        "git commit -S -n",
        "git commit -uall -n",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn the_hook_command_fails_closed_when_ns_cannot_run() {
    let s: Value = serde_json::from_str(&settings(Path::new("/nonexistent/ns"))).unwrap();
    let cmd = s["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let st = std::process::Command::new("sh")
        .args(["-c", cmd])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(st.status.code(), Some(2));
}

#[test]
fn a_deleted_binary_path_loses_the_kernel_suffix() {
    assert_eq!(
        strip_deleted(PathBuf::from("/a/ns (deleted)")),
        PathBuf::from("/a/ns")
    );
    assert_eq!(
        strip_deleted(PathBuf::from("/a/ns")),
        PathBuf::from("/a/ns")
    );
}

#[test]
fn settings_run_this_binary_on_every_tool_that_can_bypass() {
    let s: Value = serde_json::from_str(&settings(Path::new("/opt/my ns/ns"))).unwrap();
    let hook = &s["hooks"]["PreToolUse"][0];
    assert_eq!(hook["matcher"], "Bash|Edit|Write|MultiEdit|NotebookEdit");
    assert_eq!(
        hook["hooks"][0]["command"],
        "'/opt/my ns/ns' guard-hooks || exit 2"
    );
}
