use super::*;
use std::path::Path;

use crate::pin::settings_json as settings;

mod files;
mod git;
mod words;

fn check(call: &Value) -> Option<String> {
    verdict(call)
}

fn bash_call(cmd: &str) -> Value {
    json!({"tool_name": "Bash", "tool_input": {"command": cmd}})
}

fn denied(cmd: &str) -> bool {
    check(&bash_call(cmd)).is_some()
}

fn reason(cmd: &str) -> String {
    check(&bash_call(cmd)).unwrap_or_else(|| panic!("not denied: {cmd}"))
}

#[test]
fn the_denial_names_the_rule() {
    let why = reason("git commit -n -m x");
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
        "git commit -uno -m x",
        "git commit -Sn -m x",
        "timeout",
        "0=x rm .githooks/pre-commit",
        "cat <&3 .githooks/pre-commit",
        "git push origin ns/x",
        "git push -n origin ns/x",
        "git push origin -- --no-verify",
        "git merge -n main",
        "git rebase origin/main",
        "git config --get core.hooksPath",
        "git -c user.name=x commit -m y",
        "git -C repo log",
        "echo 'git commit --no-verify'",
        "echo \"GIT_DIR=x rm .githooks/pre-commit\"",
        "grep -rn no-verify .githooks/",
        "cat .githooks/pre-commit",
        "cp .githooks/pre-commit /tmp/x",
        "sed -n 1,5p .githooks/pre-push",
        "ls -la # git commit -n",
        "cargo test",
        "cat > build.md <<'EOF'\ngit commit --no-verify\nrm .githooks/pre-commit\nEOF\ngit status",
        "cat <<-EOF > x\n\tgit commit -n\n\tEOF",
        "cmd 2>&1 | tail",
        "cp a b < .githooks/pre-commit",
        "export PATH=/x:$PATH",
        "echo x > .ns/u1/review.md",
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

fn hook_exit_code(ns: &Path) -> Option<i32> {
    let s: Value = serde_json::from_str(&settings(ns)).unwrap();
    let cmd = s["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    crate::proc::Proc::tool("sh")
        .args(["-c", cmd])
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
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [("fails", "exit 1"), ("panics", "echo boom >&2; exit 101")] {
        let stub = dir.path().join(name);
        crate::testutil::write_exe(&stub, format!("#!/bin/sh\n{body}\n"));
        assert_eq!(hook_exit_code(&stub), Some(2), "{name}");
    }
}

#[test]
fn settings_run_this_binary_on_every_tool_that_can_bypass() {
    let s: Value = serde_json::from_str(&settings(Path::new("/opt/my ns/ns"))).unwrap();
    let hook = &s["hooks"]["PreToolUse"][0];
    let tools: Vec<&str> = std::iter::once(BASH_TOOL)
        .chain(FILE_TOOLS.iter().copied())
        .collect();
    assert_eq!(hook["matcher"], tools.join("|"));
    assert_eq!(
        hook["hooks"][0]["command"],
        "'/opt/my ns/ns' guard-hooks || exit 2"
    );
}

#[test]
fn each_rule_names_itself_in_the_denial() {
    for (cmd, rule) in [
        ("git commit -n", "skips the hooks"),
        ("git push --no-verify", "skips the hooks"),
        ("git -c core.hooksPath=x commit", "overrides core.hooksPath"),
        ("git --git-dir=/o log", "other hooks and config"),
        ("GIT_DIR=/tmp/x git log", "other hooks and config"),
        ("rm .githooks/pre-commit", "in .githooks/"),
        ("echo x > .githooks/pre-commit", "in .githooks/"),
        ("git rm .githooks/pre-commit", "in .githooks/"),
    ] {
        let why = reason(cmd);
        assert!(why.contains(rule), "{cmd}: {why}");
    }
    let edit = json!({"tool_name": "Edit", "tool_input": {"file_path": "/r/.githooks/x"}});
    assert!(check(&edit).unwrap().contains("in .githooks/"));
}

#[test]
fn a_bypass_after_another_command_is_denied() {
    for c in [
        "git add . ; git commit -n",
        "true && git commit -n",
        "false || git push --no-verify",
        "true | git commit -n",
        "(git commit -n)",
        "{ git commit -n; }",
        "echo $(git commit -n)",
        "cat <<EOF\nbody\nEOF\ngit commit -n",
        "cd cli && git commit -n",
        "sudo git commit -n",
        "sudo -E git commit -n",
        "env A=1 git commit -n",
        "timeout 60 git commit -n",
        "timeout --foreground 60 git commit -n",
        "/usr/bin/git commit -n",
        "A=1 B=2 git commit -n",
    ] {
        assert!(denied(c), "{c}");
    }
}
