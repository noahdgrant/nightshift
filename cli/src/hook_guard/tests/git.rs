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
        "git -C repo commit --no-verify",
        "git commit -m a#b -n",
        "git commit -m \"x\" -n",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn push_without_hooks_is_denied() {
    for c in [
        "git push --no-verify",
        "git push --no-verify origin ns/x",
        "git push origin ns/x --no-verify",
        "git push --no-v",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn hooks_path_overrides_are_denied() {
    for c in [
        "git -c core.hooksPath=/dev/null commit -m x",
        "git -c core.hookspath= push",
        "git -c CORE.HOOKSPATH=x push",
        "git -ccore.hooksPath=/dev/null commit",
        "git --config-env core.hooksPath=VAR commit",
        "git --config-env=core.hooksPath=VAR commit",
        "git -c user.name=x -c core.hooksPath=x commit",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn another_git_dir_is_denied() {
    for c in [
        "git --git-dir=/o log",
        "git --git-dir /o log",
        "git -C x --git-dir=/o for-each-ref",
        "GIT_DIR=/tmp/nonexist bash x.sh",
        "D=x; GIT_DIR=$D cargo test",
        "export GIT_DIR=/o",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_value_option_does_not_hide_the_subcommand() {
    assert!(denied("git -C commit commit -n"));
    assert!(!denied("git -C commit log -n"));
    assert!(!denied("git --work-tree commit status"));
}
