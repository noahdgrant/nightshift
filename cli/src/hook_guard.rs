//! `ns guard-hooks`: the `PreToolUse` hook `ns run` gives every claude phase. It reads the
//! tool call claude is about to make on stdin and denies one that would skip the repo's git
//! hooks, naming the rule (`skills/ns-contract`, Git hooks). Every other call passes untouched.

use std::io::Read;
use std::path::Path;

use serde_json::{json, Value};

use crate::shell::shell_quote;

const RULE: &str = "The factory contract's Git hooks rule (skills/ns-contract/SKILL.md, \"Git hooks\") says a phase never skips the repo's hooks. Fix what the hook reports and commit or push again; when the cause lies outside the unit, stop with status: blocked.";

/// The `--settings` value for a claude phase: one `PreToolUse` hook that runs `ns guard-hooks`.
pub fn settings(ns: &Path) -> String {
    let command = format!("{} guard-hooks", shell_quote(&ns.to_string_lossy()));
    json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Bash|Edit|Write|MultiEdit|NotebookEdit",
                "hooks": [{"type": "command", "command": command}],
            }],
        },
    })
    .to_string()
}

/// Read the hook input from stdin and print a deny decision for a bypass. Input that isn't a
/// tool call passes: the guard never blocks a call it can't read.
pub fn run() -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let Ok(call) = serde_json::from_str::<Value>(&input) else {
        return Ok(());
    };
    if let Some(why) = check(&call) {
        let out = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": format!("ns: blocked: {why}. {RULE}"),
            },
        });
        println!("{out}");
    }
    Ok(())
}

/// Why `call` would skip the git hooks, or `None` when it wouldn't.
pub fn check(call: &Value) -> Option<String> {
    let input = &call["tool_input"];
    match call["tool_name"].as_str()? {
        "Bash" => bash(input["command"].as_str()?),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => ["file_path", "notebook_path"]
            .iter()
            .filter_map(|k| input[k].as_str())
            .find(|p| in_hooks_dir(p))
            .map(|p| format!("edit of {p} in .githooks/")),
        _ => None,
    }
}

fn in_hooks_dir(path: &str) -> bool {
    path.split('/').any(|c| c == ".githooks")
}

#[derive(Debug, PartialEq)]
enum Tok {
    Word(String),
    /// `>` or `>>`: the next word is a file written.
    Out,
    /// `<`: the next word is a file read.
    In,
    /// `;`, `&&`, `|`, a newline, a subshell paren or a backtick: a command ends.
    Sep,
}

#[derive(Default)]
struct Lexer {
    out: Vec<Tok>,
    word: Option<String>,
    /// The next word is a heredoc delimiter.
    want_delim: bool,
    /// Delimiters of heredocs whose bodies start after the next newline.
    heredocs: Vec<String>,
}

impl Lexer {
    fn word(&mut self) -> &mut String {
        self.word.get_or_insert_with(String::new)
    }

    fn end_word(&mut self) {
        if let Some(w) = self.word.take() {
            if std::mem::take(&mut self.want_delim) {
                self.heredocs.push(w);
            } else {
                self.out.push(Tok::Word(w));
            }
        }
    }

    fn op(&mut self, t: Tok) {
        self.end_word();
        self.out.push(t);
    }
}

/// Split a shell command into words and operators. Quotes join and are removed, so a bypass
/// mentioned inside a quoted string is one word, never a command. Heredoc bodies are skipped.
fn tokens(s: &str) -> Vec<Tok> {
    let mut lx = Lexer::default();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                let w = lx.word();
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    w.push(c);
                }
            }
            '"' => {
                let w = lx.word();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => w.extend(chars.next()),
                        c => w.push(c),
                    }
                }
            }
            '\\' => {
                if let Some(c) = chars.next().filter(|c| *c != '\n') {
                    lx.word().push(c);
                }
            }
            '#' if lx.word.is_none() => while chars.next_if(|c| *c != '\n').is_some() {},
            ' ' | '\t' => lx.end_word(),
            '\n' => {
                lx.op(Tok::Sep);
                for delim in std::mem::take(&mut lx.heredocs) {
                    let mut line = String::new();
                    for c in chars.by_ref() {
                        if c != '\n' {
                            line.push(c);
                        } else if line.trim_start_matches('\t') == delim {
                            break;
                        } else {
                            line.clear();
                        }
                    }
                }
            }
            '&' if chars.next_if_eq(&'>').is_some() => {
                chars.next_if_eq(&'>');
                lx.op(Tok::Out);
            }
            ';' | '&' | '|' | '(' | ')' | '`' => lx.op(Tok::Sep),
            '{' | '}' if lx.word.is_none() => lx.op(Tok::Sep),
            '>' => {
                lx.end_word();
                chars.next_if(|c| *c == '>' || *c == '|');
                // `>&2` duplicates a descriptor; it names no file.
                if chars.next_if_eq(&'&').is_none() {
                    lx.out.push(Tok::Out);
                }
            }
            '<' => {
                lx.end_word();
                if chars.next_if_eq(&'<').is_none() || chars.next_if_eq(&'<').is_some() {
                    lx.out.push(Tok::In);
                } else {
                    chars.next_if_eq(&'-');
                    lx.want_delim = true;
                }
            }
            c => lx.word().push(c),
        }
    }
    lx.end_word();
    lx.out
}

/// Why the shell command `s` would skip the git hooks.
fn bash(s: &str) -> Option<String> {
    let toks = tokens(s);
    let mut cmd: Vec<String> = Vec::new();
    let mut toks = toks.into_iter().peekable();
    loop {
        match toks.next() {
            Some(Tok::Word(w)) => cmd.push(w),
            Some(Tok::Out) => {
                if let Some(Tok::Word(f)) = toks.next() {
                    if in_hooks_dir(&f) {
                        return Some(format!("write to {f} in .githooks/"));
                    }
                }
            }
            Some(Tok::In) => {
                toks.next();
            }
            Some(Tok::Sep) | None => {
                if let Some(why) = simple(&cmd) {
                    return Some(why);
                }
                cmd.clear();
                toks.peek()?;
            }
        }
    }
}

/// Why one simple command (no operators) would skip the git hooks.
fn simple(words: &[String]) -> Option<String> {
    let mut w = words;
    loop {
        let first = w.first()?;
        if let Some((name, value)) = assignment(first) {
            if let Some(why) = env_bypass(name, value) {
                return Some(why);
            }
            w = &w[1..];
            continue;
        }
        match base(first) {
            "export" | "env" | "sudo" | "command" | "exec" | "nohup" | "time" | "nice"
            | "xargs" => {
                w = &w[1..];
                while w.first().is_some_and(|a| a.starts_with('-')) {
                    w = &w[1..];
                }
            }
            "timeout" => {
                w = &w[1..];
                while let Some(a) = w.first().filter(|a| a.starts_with('-')) {
                    let takes_value =
                        ["-k", "-s", "--kill-after", "--signal"].contains(&a.as_str());
                    w = w.get(if takes_value { 2 } else { 1 }..)?;
                }
                w = w.get(1..)?;
            }
            "eval" => return bash(&w[1..].join(" ")),
            "sh" | "bash" | "zsh" | "dash" => {
                let i = w[1..]
                    .iter()
                    .position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c'))?;
                return bash(w.get(i + 2)?);
            }
            "git" => return git(&w[1..]),
            prog => return file_tool(prog, &w[1..]),
        }
    }
}

/// `NAME=value` as a shell assignment.
fn assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let ok = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    ok.then_some((name, value))
}

fn env_bypass(name: &str, value: &str) -> Option<String> {
    let hit = match name {
        "HUSKY" => value == "0",
        "SKIP" | "GIT_CONFIG_COUNT" | "GIT_CONFIG_PARAMETERS" => true,
        n => n.starts_with("GIT_CONFIG_KEY_") || n.starts_with("GIT_CONFIG_VALUE_"),
    };
    hit.then(|| format!("{name}= makes git hooks skip or moves core.hooksPath"))
}

fn base(prog: &str) -> &str {
    prog.rsplit('/').next().unwrap_or(prog)
}

fn is_hooks_path(key: &str) -> bool {
    key.eq_ignore_ascii_case("core.hooksPath")
}

/// `--no-verify` or an abbreviation git accepts for it.
fn no_verify(arg: &str) -> bool {
    let name = arg.split('=').next().unwrap_or(arg);
    name.len() >= "--no-v".len() && "--no-verify".starts_with(name)
}

fn git(args: &[String]) -> Option<String> {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        match a.as_str() {
            "-c" => {
                let kv = args.get(i + 1)?;
                if is_hooks_path(kv.split('=').next().unwrap_or(kv)) {
                    return Some(format!("git -c {kv} overrides core.hooksPath"));
                }
                i += 2;
            }
            "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path" => i += 2,
            o if o.starts_with("--config-env=") => {
                let key = o["--config-env=".len()..].split('=').next().unwrap_or("");
                if is_hooks_path(key) {
                    return Some(format!("git {o} overrides core.hooksPath"));
                }
                i += 1;
            }
            o if o.starts_with('-') => i += 1,
            _ => break,
        }
    }
    let sub = args.get(i)?.as_str();
    let rest = &args[i + 1..];
    match sub {
        "commit" => commit(rest),
        "push" | "merge" | "rebase" | "pull" | "am" | "cherry-pick" | "revert" => rest
            .iter()
            .find(|a| no_verify(a))
            .map(|a| format!("git {sub} {a} skips the hooks")),
        "config" => config(rest),
        "rm" | "mv" => rest
            .iter()
            .find(|a| in_hooks_dir(a))
            .map(|a| format!("git {sub} {a} in .githooks/")),
        _ => None,
    }
}

fn commit(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            return None;
        }
        if a.starts_with("--") {
            if no_verify(a) {
                return Some(format!("git commit {a} skips the hooks"));
            }
            let takes_value = [
                "--message",
                "--file",
                "--reuse-message",
                "--reedit-message",
                "--template",
                "--author",
                "--date",
                "--fixup",
                "--squash",
                "--trailer",
            ];
            if !a.contains('=') && takes_value.contains(&a.as_str()) {
                it.next();
            }
            continue;
        }
        let Some(flags) = a.strip_prefix('-') else {
            continue;
        };
        for (n, c) in flags.chars().enumerate() {
            match c {
                'n' => return Some(format!("git commit {a} skips the hooks")),
                'm' | 'F' | 'c' | 'C' | 't' => {
                    if n + 1 == flags.len() {
                        it.next();
                    }
                    break;
                }
                'u' | 'S' => break,
                _ => {}
            }
        }
    }
    None
}

fn config(args: &[String]) -> Option<String> {
    let key = args.iter().position(|a| is_hooks_path(a))?;
    let writes = [
        "set",
        "unset",
        "--unset",
        "--unset-all",
        "--add",
        "--replace-all",
        "--edit",
        "-e",
    ];
    let read_only =
        key + 1 == args.len() && !args[..key].iter().any(|a| writes.contains(&a.as_str()));
    (!read_only).then(|| "git config core.hooksPath moves the hooks".to_string())
}

/// A file command that edits, moves or deletes something in `.githooks/`.
fn file_tool(prog: &str, args: &[String]) -> Option<String> {
    let hit = |a: &&String| in_hooks_dir(a);
    let found = match prog {
        "rm" | "rmdir" | "unlink" | "shred" | "mv" | "chmod" | "chown" | "truncate" | "tee"
        | "ln" | "touch" => args.iter().find(hit),
        "cp" | "install" | "rsync" => args.last().filter(|a| in_hooks_dir(a)),
        "sed" | "perl" => args
            .iter()
            .any(|a| a.starts_with("-i") || a == "--in-place" || a.starts_with("--in-place="))
            .then(|| args.iter().find(hit))
            .flatten(),
        _ => None,
    }?;
    Some(format!("{prog} {found} in .githooks/"))
}

#[cfg(test)]
mod tests {
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
    fn settings_run_this_binary_on_every_tool_that_can_bypass() {
        let s: Value = serde_json::from_str(&settings(Path::new("/opt/my ns/ns"))).unwrap();
        let hook = &s["hooks"]["PreToolUse"][0];
        assert_eq!(hook["matcher"], "Bash|Edit|Write|MultiEdit|NotebookEdit");
        assert_eq!(hook["hooks"][0]["command"], "'/opt/my ns/ns' guard-hooks");
    }
}
