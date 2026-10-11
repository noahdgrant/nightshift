//! What the guard reads in a git command.

use super::*;

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
fn global_options_that_take_a_value_are_skipped() {
    assert!(denied("git --attr-source HEAD commit -n"));
    assert!(denied("git --attr-source=HEAD commit -n"));
    assert!(!denied("git --attr-source HEAD commit -m x"));
}

#[test]
fn git_config_section_writes_that_drop_hookspath_are_denied() {
    for c in [
        "git config --remove-section core",
        "git config --rename-section core other",
        "git config --rename-section CORE other",
        "git config --global --remove-section Core",
        "git config -e",
        "git config --edit",
        "git config --local --edit",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "git config --remove-section user",
        "git config --rename-section remote.a remote.b",
        "git config --get user.name",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn opaque_alias_arguments_okdir_and_coproc_are_read() {
    for c in [
        "git -c 'alias.x=!git commit' x $F",
        r"find .githooks -okdir true {} \;",
        r"find . -okdir git commit -n \;",
        "coproc git commit -n",
    ] {
        assert!(denied(c), "{c}");
    }
    assert!(!denied("git -c alias.x=status x $F"));
}

#[test]
fn git_config_section_writes_read_both_names_and_the_subcommand_form() {
    assert!(denied("git config --rename-section other core"));
    assert!(denied("git config edit"));
    assert!(denied("git config --global edit"));
    assert!(!denied("git config user.name edit"));
    assert!(!denied("git config alias.rm remove-section"));
}

#[test]
fn env_and_options_that_redirect_gits_config_or_dir_are_denied() {
    for c in [
        "GIT_CONFIG_GLOBAL=/tmp/c git commit -m x",
        "GIT_CONFIG_SYSTEM=/tmp/c git commit -m x",
        "GIT_DIR=/tmp/o git commit -m x",
        "git --git-dir=/tmp/o commit -m x",
        "git --git-dir /tmp/o commit -m x",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in ["HOME=/tmp git status", "XDG_CONFIG_HOME=/tmp git status"] {
        assert!(!denied(c), "{c}");
    }
}
