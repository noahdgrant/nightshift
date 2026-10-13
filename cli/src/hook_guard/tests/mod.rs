use super::*;
use std::path::Path;

use crate::pin::settings_json as settings;

mod files;
mod git;
mod resolve;
mod shell;

/// Why `call` would skip the git hooks, with no variable known.
fn check(call: &Value) -> Option<String> {
    verdict(call, &Vars::new(), Unread::Keep)
}

fn bash_call(cmd: &str) -> Value {
    json!({"tool_name": "Bash", "tool_input": {"command": cmd}})
}

fn denied(cmd: &str) -> bool {
    check(&bash_call(cmd)).is_some()
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
    assert_eq!(hook["matcher"], "Bash|Edit|Write|MultiEdit|NotebookEdit");
    let tools: Vec<&str> = std::iter::once(BASH_TOOL)
        .chain(FILE_TOOLS.iter().copied())
        .collect();
    assert_eq!(hook["matcher"], tools.join("|"));
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

/// The phase transcripts' real denials, reduced to the part that matters.
#[test]
fn real_denials_stay_denied_and_unit_artifact_writes_pass() {
    for c in [
        "U=u1 && S=$(git rev-parse --short HEAD) && cat > .ns/$U/review.md <<EOF\nunit: $U\nEOF",
        "U=u1\ncat > .ns/$U/pr.md <<'EOF'\nbody\nEOF",
        "U=u1; cat > .ns/$U/evidence.md <<EOF\nx\nEOF",
        "U=u1\ncat > .ns/$U/brief.md <<'EOF'\nx\nEOF",
        "{ printf -- \"---\\nunit: $U\\n\"; cat /tmp/body.md; } > .ns/$U/pr.md",
        "NOW=$(date -u +%FT%TZ) && cat > .ns/$U/review.md <<E\nx\nE",
        "git push -q \"$V/remote.git\" main",
    ] {
        assert!(!denied(c), "{c}");
    }
    for c in [
        "git commit -n -m x",
        "git commit --no-verify -m x",
        "git push --no-verify",
        "D=$(mktemp -d); GIT_DIR=$D cargo test",
        "GIT_DIR=/tmp/nonexist bash x.sh",
        "git --git-dir=/o log",
        "git --git-dir=/tmp for-each-ref",
        "mut(){ d=$(mktemp -d); (cd $d && eval \"$1\"); }",
        "sh -c \"$X\"",
        "$G commit -n",
        "X=n; git commit -a$X -m x",
        "cd \"$(dirname $NS_PHASE_SETTINGS)\" && rm phase-settings.json",
    ] {
        assert!(denied(c), "{c}");
    }
}
