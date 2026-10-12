//! Every external command `cli/tests` and the rule packs' `tests/run.sh` run is pinned in
//! `scripts/ci-tools.d/` or is a base tool every CI runner has, so CI installs what the
//! tests need.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// Tools every runner and contributor machine has, so the list doesn't pin them.
const BASE_TOOLS: &[&str] = &[
    "bash", "basename", "cat", "chmod", "cmp", "cp", "dirname", "flock", "git", "grep", "jq", "ln",
    "mkdir", "mkfifo", "mktemp", "ps", "python3", "rm", "sed", "sh", "tee", "timeout", "tr",
];

/// Shell builtins and keywords that start a command but run nothing external.
const BUILTINS: &[&str] = &[
    "break", "cd", "command", "continue", "echo", "eval", "exec", "exit", "export", "false",
    "local", "printf", "pwd", "read", "return", "set", "shift", "shopt", "test", "trap", "true",
    "unset", "wait",
];

/// Words that come before a command: the command is the next word.
const PREFIXES: &[&str] = &[
    "!", "{", "}", "do", "elif", "else", "if", "then", "time", "until", "while",
];

/// Words that start something other than a command.
const NOT_COMMANDS: &[&str] = &[
    "case", "done", "esac", "fi", "for", "function", "in", "select",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The tools pinned in `scripts/ci-tools.d/`, one `<tool>.toml` each.
fn listed() -> BTreeSet<String> {
    fs::read_dir(root().join("scripts/ci-tools.d"))
        .unwrap()
        .filter_map(|e| {
            let path = e.unwrap().path();
            let tool = path.file_stem()?.to_str()?.to_owned();
            (path.extension()? == "toml").then_some(tool)
        })
        .collect()
}

/// The programs a Rust test starts by name, a string literal passed to `Command::new` or
/// `common::command`.
fn rust_commands(src: &str) -> Vec<String> {
    let re = Regex::new(r#"(?:Command::new|\bcommand)\("([^"\\]+)"\)"#).unwrap();
    re.captures_iter(src).map(|c| c[1].to_owned()).collect()
}

/// `src` with the parts that can't start a command blanked: quoted text (but not a `$(...)`
/// inside double quotes), `${...}`, `$((...))`, comments and here-documents. Every character
/// that can end a command becomes a `;` or a newline.
fn flatten(src: &str) -> String {
    enum State {
        /// Unquoted code, with the count of `(` it has opened.
        Code(usize),
        Double,
    }
    let cs: Vec<char> = src.chars().collect();
    let at = |i: usize, s: &str| cs[i..].starts_with(&s.chars().collect::<Vec<_>>());
    let mut stack = vec![State::Code(0)];
    let mut out = String::new();
    let mut heredoc: Option<String> = None;
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let nested = stack.len() > 1;
        match stack.last_mut().unwrap() {
            State::Double => match c {
                '\\' => i += 1,
                '"' => {
                    stack.pop();
                    out.push('"');
                }
                '$' if at(i, "$((") => i = skip_balanced(&cs, i + 1, '(', ')'),
                '$' if at(i, "$(") => {
                    stack.push(State::Code(0));
                    out.push(';');
                    i += 1;
                }
                _ => {}
            },
            State::Code(depth) => match c {
                '\\' => {
                    out.push(' ');
                    i += 1;
                }
                '\'' => {
                    i += cs[i + 1..]
                        .iter()
                        .position(|&q| q == '\'')
                        .map_or(cs.len(), |p| p + 1);
                    out.push_str("''");
                }
                '"' => {
                    stack.push(State::Double);
                    out.push('"');
                }
                '#' if i == 0 || cs[i - 1].is_whitespace() || ";|&(".contains(cs[i - 1]) => {
                    i += cs[i..]
                        .iter()
                        .position(|&c| c == '\n')
                        .unwrap_or(cs.len() - i)
                        - 1;
                }
                '$' if at(i, "$((") => {
                    i = skip_balanced(&cs, i + 1, '(', ')');
                    out.push_str("$x");
                }
                '$' if at(i, "${") => {
                    i = skip_balanced(&cs, i + 1, '{', '}');
                    out.push_str("$x");
                }
                '$' if at(i, "$(") => {
                    *depth += 1;
                    out.push(';');
                    i += 1;
                }
                '(' | '`' => {
                    *depth += 1;
                    out.push(';');
                }
                ')' => {
                    if *depth == 0 && nested {
                        stack.pop();
                    } else {
                        *depth = depth.saturating_sub(1);
                    }
                    out.push(';');
                }
                '<' if at(i, "<<<") => {
                    out.push_str("<<<");
                    i += 2;
                }
                '<' if at(i, "<<") => {
                    let (word, next) = heredoc_word(&cs, i + 2);
                    heredoc = Some(word);
                    out.push(' ');
                    i = next;
                    continue;
                }
                '\n' => {
                    out.push('\n');
                    if let Some(end) = heredoc.take() {
                        i = heredoc_end(&cs, i, &end);
                    }
                }
                ';' | '|' | '&' => out.push(';'),
                _ => out.push(c),
            },
        }
        i += 1;
    }
    out
}

/// The terminator word of a here-document whose `<<` ends just before `cs[i]`, and the index
/// after it.
fn heredoc_word(cs: &[char], mut i: usize) -> (String, usize) {
    while i < cs.len() && "-'\" ".contains(cs[i]) {
        i += 1;
    }
    let start = i;
    while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
        i += 1;
    }
    let word = cs[start..i].iter().collect();
    while i < cs.len() && "'\"".contains(cs[i]) {
        i += 1;
    }
    (word, i)
}

/// The index of the newline ending the first line after `cs[i]` that is `end` alone.
fn heredoc_end(cs: &[char], mut i: usize, end: &str) -> usize {
    while i < cs.len() {
        let start = i + 1;
        i = cs[start..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(cs.len(), |p| start + p);
        if cs[start..i].iter().collect::<String>().trim() == end {
            break;
        }
    }
    i
}

/// The index of the `close` that balances the `open` at `cs[i]`.
fn skip_balanced(cs: &[char], mut i: usize, open: char, close: char) -> usize {
    let mut depth = 0;
    while i < cs.len() {
        match cs[i] {
            '\\' => i += 1,
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

/// The external programs a shell script runs by name, leaving out builtins, the script's own
/// functions and commands it can't name until it runs (`"$x"`, `./x`).
fn shell_commands(src: &str) -> Vec<String> {
    let functions: BTreeSet<&str> = Regex::new(r"(?m)^\s*([A-Za-z_][\w-]*)\s*\(\)")
        .unwrap()
        .captures_iter(src)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    let assignment = Regex::new(r"^[A-Za-z_]\w*=").unwrap();
    let name = Regex::new(r"^[A-Za-z_][\w.+-]*$").unwrap();
    let mut out = Vec::new();
    for segment in flatten(src).split([';', '\n']) {
        let first = segment
            .split_whitespace()
            .find(|w| !PREFIXES.contains(w) && !assignment.is_match(w));
        if let Some(w) = first {
            if name.is_match(w)
                && !NOT_COMMANDS.contains(&w)
                && !BUILTINS.contains(&w)
                && !functions.contains(w)
            {
                out.push(w.to_owned());
            }
        }
    }
    out
}

/// Each command in `files` that is neither listed nor a base tool, as `file: command`.
fn unlisted(files: &[(String, String)]) -> Vec<String> {
    let listed = listed();
    let mut bad = Vec::new();
    for (path, src) in files {
        let commands = if path.ends_with(".rs") {
            rust_commands(src)
        } else {
            shell_commands(src)
        };
        for c in commands {
            if !listed.contains(&c) && !BASE_TOOLS.contains(&c.as_str()) {
                bad.push(format!("{path}: {c}"));
            }
        }
    }
    bad.sort();
    bad.dedup();
    bad
}

/// `cli/tests/**/*.rs` and `rules/*/tests/run.sh`, with their text.
fn test_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = root();
    let mut paths = Vec::new();
    walk(&root.join("cli/tests"), &mut paths);
    for pack in fs::read_dir(root.join("rules")).unwrap() {
        let run = pack.unwrap().path().join("tests/run.sh");
        if run.is_file() {
            paths.push(run);
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let rel = p.strip_prefix(&root).unwrap().display().to_string();
            (rel, fs::read_to_string(&p).unwrap())
        })
        .collect()
}

#[test]
fn every_command_the_tests_run_is_listed_or_a_base_tool() {
    let sources = test_sources();
    assert!(sources
        .iter()
        .any(|(p, _)| p == "rules/python/tests/run.sh"));
    assert!(sources.iter().any(|(p, _)| p == "cli/tests/common/mod.rs"));
    let bad = unlisted(&sources);
    assert!(
        bad.is_empty(),
        "these commands are neither in scripts/ci-tools.d/ nor in BASE_TOOLS, so CI may not \
         install them; pin each in scripts/ci-tools.d/:\n{}",
        bad.join("\n")
    );
}

#[test]
fn an_unlisted_command_fails_and_a_listed_one_passes() {
    let files = [
        (
            "cli/tests/x.rs".to_owned(),
            "StdCommand::new(\"shellcheck\"); common::command(\"ruff\"); Command::new(\"git\"); \
             Command::new(tool); mycommand(\"nope\"); Command::new(\"shellcheck\")"
                .to_owned(),
        ),
        (
            "rules/x/tests/run.sh".to_owned(),
            "shellcheck a.sh\nruff check .\n".to_owned(),
        ),
    ];
    assert_eq!(
        unlisted(&files),
        [
            "cli/tests/x.rs: shellcheck",
            "rules/x/tests/run.sh: shellcheck"
        ]
    );
}

#[test]
fn the_shell_scan_finds_commands_where_shell_runs_them() {
    let src = r#"#!/usr/bin/env bash
set -euo pipefail  # a comment naming nocomment
helper() { one "$@"; }
x=$(two -c "three; four")
if three; then four | five && six || seven; fi
while IFS= read -r line; do eight; done < <(nine)
echo "text $(ten) more" >&2
n=$((eleven + 1)); v=${twelve:-x}
'thirteen' "fourteen" "$fifteen" ./sixteen
cat <<'END_TEXT'
seventeen
END_TEXT
case $v in pat*) eighteen ;; esac
FOO=1 nineteen \
  twenty
helper
v=`twentyone`; tr a b <<<"$v"
twentytwo "\"$(twentythree)\" $(( twentyfour ))" $#; twentyfive
echo "$( (twentysix); twentyseven )" ${a//\}/;bogus;}
"#;
    assert_eq!(
        shell_commands(src),
        [
            "one",
            "two",
            "three",
            "four",
            "five",
            "six",
            "seven",
            "eight",
            "nine",
            "ten",
            "cat",
            "eighteen",
            "nineteen",
            "twentyone",
            "tr",
            "twentytwo",
            "twentythree",
            "twentyfive",
            "twentysix",
            "twentyseven"
        ]
    );
}

fn ci_workflow() -> String {
    fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap()
}

/// The tools each `scripts/ci-tools.sh install ...` line in `text` installs.
fn installed_by(text: &str) -> BTreeSet<String> {
    let re = Regex::new(r"ci-tools\.sh install([^\n]*)").unwrap();
    re.captures_iter(text)
        .flat_map(|c| {
            c[1].split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn ci_pins_no_tool_version_outside_the_list() {
    let pin = Regex::new(r"[A-Za-z0-9_.-]+(==|@v?)\d+(\.\d+)+").unwrap();
    let bad: Vec<String> = ci_workflow()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.contains("uses:"))
        .filter(|l| pin.is_match(l))
        .map(|l| l.trim().to_owned())
        .collect();
    assert!(
        bad.is_empty(),
        "pin these in scripts/ci-tools.d/ instead:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_job_that_runs_cargo_test_installs_every_listed_tool_the_tests_name() {
    let yml = ci_workflow();
    let jobs = Regex::new(r"(?m)^  [a-z][\w-]*:\s*$").unwrap();
    let job = jobs
        .split(&yml)
        .find(|j| j.contains("- run: cargo test\n"))
        .expect("a job runs cargo test");
    let installed = installed_by(job);
    let listed = listed();
    let mut needed = BTreeSet::new();
    for (path, src) in test_sources() {
        let stubs = ["test_tools.rs", "ci_tools.rs", "ci_local.rs"];
        if path.starts_with("cli/tests/") && !stubs.iter().any(|f| path.ends_with(f)) {
            needed.extend(listed.iter().filter(|t| {
                let word = format!(r"(^|[^\w-]){}($|[^\w-])", regex::escape(t));
                Regex::new(&word).unwrap().is_match(&src)
            }));
        }
    }
    assert!(needed.contains(&"ast-grep".to_owned()), "{needed:?}");
    let missing: Vec<_> = needed
        .into_iter()
        .filter(|t| !installed.contains(*t))
        .collect();
    assert!(
        missing.is_empty(),
        "the cargo test job must run `scripts/ci-tools.sh install` for: {missing:?}"
    );
}

/// Each rule pack (a `rules/<pack>/` with a `tests/run.sh`) and the tools in its `TOOLS`, the
/// pins the `rule-packs` job installs for it. A pack with no `TOOLS` has an empty list.
fn pack_tools() -> Vec<(String, Vec<String>)> {
    let mut packs: Vec<_> = fs::read_dir(root().join("rules"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("tests/run.sh").is_file())
        .map(|p| {
            let tools = fs::read_to_string(p.join("TOOLS")).unwrap_or_default();
            let tools = tools.split_whitespace().map(str::to_owned).collect();
            (p.file_name().unwrap().to_string_lossy().into_owned(), tools)
        })
        .collect();
    packs.sort();
    packs
}

#[test]
fn every_rule_pack_names_its_tools_and_each_is_pinned() {
    let listed = listed();
    let packs = pack_tools();
    assert!(packs.iter().any(|(p, _)| p == "python"), "{packs:?}");
    for (pack, tools) in packs {
        assert!(
            !tools.is_empty(),
            "rules/{pack}/TOOLS must name the pinned tools its tests need"
        );
        let unpinned: Vec<_> = tools.iter().filter(|t| !listed.contains(*t)).collect();
        assert!(
            unpinned.is_empty(),
            "rules/{pack}/TOOLS names tools with no pin in scripts/ci-tools.d/: {unpinned:?}"
        );
    }
}

/// The text of the job `name` in the CI workflow.
fn ci_job(name: &str) -> String {
    let yml = ci_workflow();
    let start = format!("\n  {name}:\n");
    let at = yml
        .find(&start)
        .unwrap_or_else(|| panic!("ci.yml has no job {name}"))
        + 1;
    let jobs = Regex::new(r"(?m)^  [a-z][\w-]*:\s*$").unwrap();
    let end = jobs
        .find_at(&yml, at + start.len() - 1)
        .map_or(yml.len(), |m| m.start());
    yml[at..end].to_owned()
}

#[test]
fn one_matrix_job_tests_every_rule_pack_scripts_rule_packs_sh_lists() {
    let yml = ci_workflow();
    let listing = ci_job("rule-pack-list");
    assert!(listing.contains("scripts/rule-packs.sh"), "{listing}");
    let job = ci_job("rule-packs");
    for want in [
        "needs: rule-pack-list",
        "fromJSON(needs.rule-pack-list.outputs.packs)",
        "name: Rule pack tests (${{ matrix.name }})",
        "PACK: ${{ matrix.pack }}",
        r#"scripts/ci-tools.sh install $(cat "rules/$PACK/TOOLS")"#,
        r#"scripts/check-git-dir-decoy.sh "rules/$PACK/tests/run.sh""#,
    ] {
        assert!(
            job.contains(want),
            "the rule-packs job has no {want:?}:\n{job}"
        );
    }
    let run = r#""rules/$PACK/tests/run.sh""#;
    assert!(
        job.lines().any(|l| l.trim() == run),
        "the rule-packs job doesn't run {run}:\n{job}"
    );
    let named = Regex::new(r"rules/[a-z]+/tests").unwrap();
    let hits: Vec<_> = named.find_iter(&yml).map(|m| m.as_str()).collect();
    assert!(hits.is_empty(), "ci.yml names a pack: {hits:?}");
}

#[test]
fn ci_installs_every_listed_tool_somewhere() {
    let mut installed = installed_by(&ci_workflow());
    installed.extend(pack_tools().into_iter().flat_map(|(_, tools)| tools));
    let unused: Vec<_> = listed().difference(&installed).cloned().collect();
    assert!(unused.is_empty(), "no job installs these tools: {unused:?}");
}
