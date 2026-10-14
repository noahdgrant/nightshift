//! Shell syntax, wrappers, expansions and environment variables.

use super::*;
use crate::shell::shell_quote;

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
fn heredoc_after_the_body_is_still_checked() {
    assert!(denied("cat > x <<EOF\nhi\nEOF\ngit commit -n"));
}

#[test]
fn a_shift_is_not_a_heredoc() {
    for c in [
        "echo $((1<<2))\ngit commit -n",
        "(( x = 1 << 2 ))\ngit commit -n",
        "let x=1<<2\ngit commit -n",
        "x=$((1<<2))\ngit commit -n",
        "echo $((1<<n))\ngit commit -n",
        "echo $((a<<b))\ngit commit -n",
        "echo $((x<<$y))\ngit commit -n",
        "(( x = a << b ))\ngit commit -n",
    ] {
        assert!(denied(c), "{c}");
    }
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
        "git push origin \"$branch\"",
        "git push origin $branch",
        "git merge \"$base\"",
        "git rebase \"$base\"",
        "git pull origin \"$b\"",
        "git cherry-pick \"$sha\"",
        "\"$ROOT/scripts/check-private.sh\"",
    ] {
        assert!(!denied(c), "{c}");
    }
    for c in [
        "git push $branch",
        "git push --$X",
        "git push origin -$X",
        "git push $(echo --no-verify)",
        "git merge $F",
        "git commit origin \"$F\"",
    ] {
        assert!(denied(c), "{c}");
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
fn braced_expansion_bodies_do_not_split_or_hide_the_command() {
    for c in [
        "git commit -m ${x:-a;b} -n",
        "git commit -m ${x:-a&b} -n",
        "git commit -m ${x//;/} -n",
        "git commit -m ${x#;} -n",
        "git commit -m \"${x:-\")\"}\" -n",
        "git commit -m ${x:-${y:-a;b}} -n",
        "git commit -m ${x:-$(git commit -n)}",
        "git commit ${x:-a;b}",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git commit -m ${x:-a;b}",
        "echo ${x:-a;b}; git status",
        "git commit -m \"${x:-a;b}\"",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn bash_dash_c_reads_the_script_past_options() {
    for c in [
        "bash -c -- 'git commit -n'",
        "bash -c -O extglob 'git commit -n'",
        "bash -c -o pipefail 'git commit -n'",
        "bash -c +x 'git commit -n'",
    ] {
        assert!(denied(c), "{c}");
    }
    assert!(!denied("bash -c -- 'git status'"));
}

#[test]
fn a_shell_fed_a_heredoc_or_herestring_is_denied() {
    for c in [
        "bash <<< 'git commit -n'",
        "bash <<'X'\ngit commit -n\nX",
        "sh <<EOF\ngit commit -n\nEOF",
        "sudo bash <<< 'x'",
        "bash -x <<< 'x'",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "bash -c 'git status' <<< 'x'",
        "cat <<< 'git commit -n'",
        "cat <<'X'\ngit commit -n\nX",
        "bash script.sh\ncat <<< x",
        "bash\necho <<< x",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn wrapper_values_quoted_mentions_and_alias_case() {
    assert!(denied("sudo -ugroup git commit -n"));
    assert!(denied("sudo -u group git commit -n"));
    assert!(denied("git -c alias.CI=commit ci -n"));
    for c in [
        "printf '%s\\n' 'git commit -n'",
        "grep -r 'git commit --no-verify' docs",
        "grep -- --no-verify notes.md",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn heredoc_stdin_reaches_nested_shells_and_ends_with_its_command() {
    assert!(denied("bash -c 'bash' <<< 'git commit -n'"));
    assert!(denied("eval bash <<< 'git commit -n'"));
    assert!(!denied("cat <<< x; bash script.sh"));
    assert!(!denied("eval 'cat <<< x'; bash script.sh"));
}

#[test]
fn a_line_continuation_before_the_command_does_not_hide_it() {
    for c in [
        "cd x && \\\n  git commit --no-verify -m x",
        "FOO=1 \\\n git commit -n",
        "\\\ngit commit -n",
    ] {
        assert!(denied(c), "{c}");
    }
    assert!(!denied("cd x && \\\n  git commit -m x"));
}

#[test]
fn an_expansion_inside_a_command_name_or_flag_is_opaque() {
    for c in [
        "git${IFS}commit${IFS}--no-verify",
        "gi$X commit -n",
        "X=n; git commit -a$X -m x",
        "git commit -$X",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git commit --message=$X",
        "git commit -m $X",
        "git commit -m x$Y",
        "echo a$X",
    ] {
        assert!(!denied(c), "{c}");
    }
}
