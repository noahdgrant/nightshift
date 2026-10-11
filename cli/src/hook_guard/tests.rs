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

fn nested(levels: usize) -> String {
    let mut c = "git status".to_string();
    for _ in 0..levels {
        c = format!("sh -c {}", shell_quote(&c));
    }
    c
}

#[test]
fn nesting_is_allowed_below_the_cap_and_denied_at_it() {
    assert!(!denied(&nested(MAX_NESTING - 1)));
    let why = check(&bash_call(&nested(MAX_NESTING))).expect("denied at the cap");
    assert!(why.contains("nested more than"), "{why}");
    assert!(why.contains(&MAX_NESTING.to_string()), "{why}");
    assert!(denied(&nested(MAX_NESTING + 4)));
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

fn hook_exit_code(ns: &Path) -> Option<i32> {
    let s: Value = serde_json::from_str(&settings(ns)).unwrap();
    let cmd = s["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    std::process::Command::new("sh")
        .args(["-c", cmd])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
        .status
        .code()
}

#[test]
fn the_hook_command_fails_closed_when_ns_cannot_run() {
    assert_eq!(hook_exit_code(Path::new("/nonexistent/ns")), Some(2));
}

#[test]
fn the_hook_command_fails_closed_when_ns_crashes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [("fails", "exit 1"), ("panics", "echo boom >&2; exit 101")] {
        let stub = dir.path().join(name);
        std::fs::write(&stub, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(hook_exit_code(&stub), Some(2), "{name}");
    }
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

fn reason(cmd: &str) -> String {
    check(&bash_call(cmd)).unwrap_or_else(|| panic!("not denied: {cmd}"))
}

#[test]
fn each_rule_names_itself_in_the_denial() {
    for (cmd, rule) in [
        ("git commit -n", "skips the hooks"),
        ("git push --no-verify", "skips the hooks"),
        ("git merge --no-verify x", "skips the hooks"),
        ("git -c core.hooksPath=x commit", "overrides core.hooksPath"),
        (
            "git --config-env core.hooksPath=V commit",
            "overrides core.hooksPath",
        ),
        (
            "git config core.hooksPath x",
            "core.hooksPath moves the hooks",
        ),
        ("HUSKY=0 git commit", "makes git hooks skip"),
        ("SKIP=a git commit", "makes git hooks skip"),
        ("GIT_CONFIG_COUNT=1 git commit", "makes git hooks skip"),
        ("rm .githooks/pre-commit", "in .githooks/"),
        ("echo x > .githooks/pre-commit", "in .githooks/"),
        ("git rm .githooks/pre-commit", "in .githooks/"),
        ("git commit $FLAG", "built by a shell expansion"),
        ("$G commit -n", "named by a shell expansion"),
    ] {
        let why = reason(cmd);
        assert!(why.contains(rule), "{cmd}: {why}");
    }
    let edit = json!({"tool_name": "Edit", "tool_input": {"file_path": "/r/.githooks/x"}});
    assert!(check(&edit).unwrap().contains("in .githooks/"));
}

#[test]
fn standalone_env_forms_are_denied() {
    for c in [
        "GIT_CONFIG_COUNT=1 git commit",
        "env -Sgit\\ commit\\ -n",
        "env -S 'git commit -n'",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn every_wrapper_option_that_takes_a_value_is_skipped() {
    for w in WRAPPERS {
        let tail = format!("{}git commit -n", "5 ".repeat(w.positional));
        let mut forms = vec![format!("{} {tail}", w.name)];
        for c in w.short_values.chars() {
            forms.push(format!("{} -{c} V {tail}", w.name));
        }
        for l in w.long_values {
            forms.push(format!("{} {l} V {tail}", w.name));
            forms.push(format!("{} {l}=V {tail}", w.name));
        }
        for c in forms {
            let why = check(&bash_call(&c)).unwrap_or_else(|| panic!("not denied: {c}"));
            assert!(why.contains("skips the hooks"), "{c}: {why}");
        }
    }
}

#[test]
fn nice_adjustment_and_env_long_options_are_skipped() {
    for c in [
        "nice --adjustment=5 git commit -n",
        "env --unset FOO git commit -n",
        "env --unset=FOO git commit -n",
        "env --chdir /x git commit -n",
        "ionice -c 2 -n 4 git commit -n",
        "builtin command git commit -n",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn an_alias_set_with_c_is_checked_with_its_arguments() {
    for c in [
        "git -c alias.ci=commit ci -n",
        "git -c alias.c=commit c --no-verify",
        "git -c 'alias.x=!git commit' x -n",
        "git -c 'alias.x=!git commit' x --no-verify -m y",
        "git -c alias.p='push origin' p --no-verify",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git -c alias.ci=commit ci -m x",
        "git -c 'alias.x=!git commit' x -m y",
        "git -c alias.ci=commit status",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn git_commands_that_run_a_command_string_are_read() {
    for c in [
        "git rebase -x 'git commit -n' HEAD~1",
        "git rebase --exec 'git commit -n'",
        "git rebase --exec='git commit -n'",
        "git rebase -i -x 'HUSKY=0 git push'",
        "git rebase -x'git commit -n' HEAD~1",
        "git submodule foreach 'git commit -n'",
        "git submodule foreach --recursive 'git commit -n'",
        "git submodule foreach \"$CMD\"",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git rebase -x 'cargo test' HEAD~1",
        "git rebase --exec 'cargo test'",
        "git submodule foreach 'git status'",
        "git submodule update --init",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn config_writes_that_set_up_a_bypass_are_denied() {
    for c in [
        "git config alias.ci 'commit -n'",
        "git config --global alias.ci 'commit --no-verify'",
        "git config alias.x '!git commit'",
        "git config include.path /tmp/f",
        "git config --global includeIf.gitdir:/x/.path /tmp/f",
        "git config --add include.path /tmp/f",
        "git -c include.path=/tmp/f commit -m x",
        "git -c includeIf.gitdir:/x/.path=/tmp/f commit -m x",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git config alias.ci commit",
        "git config alias.st 'status -s'",
        "git config --get alias.ci",
        "git config alias.ci",
        "git config --get include.path",
        "git config --get core.hooksPath --global",
        "git config core.hooksPath --global",
        "git config --list",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn env_options_are_walked_with_their_values() {
    for c in [
        "env -iS 'git commit -n'",
        "env -u FOO -S 'git commit -n'",
        "env -uFOO -S 'git commit -n'",
        "env -C /x -S 'git commit -n'",
        "env -i -u FOO -C /x --split-string 'git commit -n'",
        "env -iSgit\\ commit\\ -n",
        "env -i -S 'HUSKY=0 git push'",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "env -u FOO -S 'cargo test'",
        "env -iS 'cargo test'",
        "env -i cargo test",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn declaring_an_exported_skip_variable_is_denied() {
    for c in [
        "declare -x HUSKY=0",
        "typeset -x SKIP=lint",
        "readonly HUSKY=0",
        "local -x SKIP=all",
        "declare -x GIT_CONFIG_COUNT=1",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in ["declare -x A=1", "readonly HUSKY=1", "declare -p"] {
        assert!(!denied(c), "{c}");
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
fn global_options_that_take_a_value_are_skipped() {
    assert!(denied("git --attr-source HEAD commit -n"));
    assert!(denied("git --attr-source=HEAD commit -n"));
    assert!(!denied("git --attr-source HEAD commit -m x"));
}
