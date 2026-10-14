//! `scripts/ci-affected.map`, the map `scripts/ci-affected.sh` picks cargo test binaries from:
//! every repo path a test binary's sources name must be one the map lists for that binary.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::git;

fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The map's entries, `(pattern, targets)`, in file order. A pattern is a repo path, or a prefix
/// ending in `*`; the targets are `all`, or cargo test target flags (`--test cli`, `--bin ns`).
fn entries(text: &str) -> Vec<(String, Vec<String>)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut words = l.split_whitespace().map(str::to_owned);
            (words.next().unwrap(), words.collect())
        })
        .collect()
}

/// The targets of an entry's `words`, as `--test <name>` or `--bin ns`, one string each, or
/// `all`.
fn targets(words: &[String]) -> Vec<String> {
    words.chunks(2).map(|c| c.join(" ")).collect()
}

/// Whether `pattern` covers every file `path` names: the path itself, or a directory with all
/// under it.
fn covers(pattern: &str, path: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => path.starts_with(prefix) || format!("{path}/").starts_with(prefix),
        None => pattern == path,
    }
}

/// The repo's tracked top-level entries, the names a repo path starts with.
fn top_level() -> BTreeSet<String> {
    git(&checkout(), &["ls-files"])
        .lines()
        .map(|f| f.split('/').next().unwrap().to_owned())
        .collect()
}

/// Rust source `src` lexed: a copy with every comment, string and char literal blanked to
/// spaces, so braces and names in it are code, and each string literal (raw ones too) with its
/// offset.
fn lex(src: &str) -> (String, Vec<(usize, String)>) {
    let b = src.as_bytes();
    let mut code = b.to_vec();
    let mut literals = Vec::new();
    let mut i = 0;
    let blank = |code: &mut Vec<u8>, from: usize, to: usize| {
        for c in &mut code[from..to.min(b.len())] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    while i < b.len() {
        let start = i;
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'r' if matches!(b.get(i + 1), Some(b'#' | b'"'))
                && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) =>
            {
                let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
                let open = i + 1 + hashes;
                if b.get(open) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let close = format!("\"{}", "#".repeat(hashes));
                let end = src[open + 1..]
                    .find(&close)
                    .map_or(b.len(), |e| e + open + 1);
                literals.push((start, src[open + 1..end].to_owned()));
                i = end + close.len();
            }
            b'"' => {
                let mut s = String::new();
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    if let Some(&c) = b.get(i) {
                        s.push(c as char);
                    }
                    i += 1;
                }
                literals.push((start, s));
                i += 1;
            }
            b'\'' if b.get(i + 1) == Some(&b'\\') => {
                i += 3;
                while i < b.len() && b[i] != b'\'' {
                    i += 1;
                }
                i += 1;
            }
            b'\'' if b.get(i + 2) == Some(&b'\'') => i += 3,
            _ => {
                i += 1;
                continue;
            }
        }
        blank(&mut code, start, i);
    }
    (String::from_utf8_lossy(&code).into_owned(), literals)
}

/// The end of the block that opens at `code[open]`, a `{`, past its `}`.
fn block_end(code: &str, open: usize) -> usize {
    let mut depth = 0;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 1 => return open + i + 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    code.len()
}

fn ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Each `fn` with a body in lexed `code`: its name, its signature, and its span.
fn fns(code: &str) -> Vec<(String, String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    for (at, _) in code.match_indices("fn ") {
        if code[..at].chars().next_back().is_some_and(ident) {
            continue;
        }
        let name: String = code[at + 3..].chars().take_while(|&c| ident(c)).collect();
        let rest = &code[at..];
        let Some(open) = rest
            .find('{')
            .filter(|&o| rest.find(';').is_none_or(|s| o < s))
        else {
            continue;
        };
        out.push((
            name,
            rest[..open].to_owned(),
            at..block_end(code, at + open),
        ));
    }
    out
}

/// Whether `text` calls `name`: `name(` with no identifier character before it.
fn calls(text: &str, name: &str) -> bool {
    text.match_indices(&format!("{name}("))
        .any(|(at, _)| !text[..at].chars().next_back().is_some_and(ident))
}

/// The string literals of the Rust sources `srcs` that can name a checkout path: those inside a
/// function that reaches the checkout, by `env!("CARGO_MANIFEST_DIR")` or by calling a function
/// that reaches it and returns a path, and those anywhere that start with `../`, a path from
/// `cli/`.
fn checkout_literals(srcs: &[String]) -> Vec<String> {
    struct Fn<'a> {
        name: String,
        returns_path: bool,
        code: &'a str,
        literals: Vec<&'a (usize, String)>,
    }
    let lexed: Vec<_> = srcs.iter().map(|s| lex(s)).collect();
    let all: Vec<Fn> = lexed
        .iter()
        .flat_map(|(code, literals)| {
            fns(code).into_iter().map(move |(name, sig, span)| Fn {
                name,
                returns_path: sig.contains("-> ") && sig.contains("Path"),
                code: &code[span.clone()],
                literals: literals
                    .iter()
                    .filter(|(at, _)| span.contains(at))
                    .collect(),
            })
        })
        .collect();
    let mut reach: Vec<bool> = lexed
        .iter()
        .flat_map(|(code, literals)| {
            fns(code).into_iter().map(move |(_, _, span)| {
                literals.iter().any(|(at, l)| {
                    span.contains(at)
                        && l == "CARGO_MANIFEST_DIR"
                        && code[..*at].trim_end().ends_with("env!(")
                })
            })
        })
        .collect();
    loop {
        let next: Vec<bool> = all
            .iter()
            .zip(&reach)
            .map(|(f, &r)| {
                r || all
                    .iter()
                    .zip(&reach)
                    .any(|(g, &rg)| rg && g.returns_path && calls(f.code, &g.name))
            })
            .collect();
        if next == reach {
            break;
        }
        reach = next;
    }
    let mut out: Vec<String> = all
        .iter()
        .zip(&reach)
        .filter(|(_, &r)| r)
        .flat_map(|(f, _)| f.literals.iter().map(|(_, l)| l.clone()))
        .collect();
    out.extend(
        lexed
            .iter()
            .flat_map(|(_, literals)| literals)
            .filter(|(_, l)| l.starts_with("../"))
            .map(|(_, l)| l.clone()),
    );
    out
}

/// The repo paths that `literals` name: each word of a literal, with any leading
/// `./` or `../` removed, cut at the first `{`, `*` or `$`, whose first component is a tracked
/// top-level entry and which exists in the checkout.
fn repo_paths(literals: &[String], top: &BTreeSet<String>) -> BTreeSet<String> {
    let root = checkout();
    literals
        .iter()
        .flat_map(|l| {
            l.split(|c: char| c.is_whitespace() || "'\"`()=:,;<>|&".contains(c))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter_map(|w| {
            let mut w = w.as_str();
            while let Some(rest) = w.strip_prefix("../").or_else(|| w.strip_prefix("./")) {
                w = rest;
            }
            let w = w
                .split(['{', '*', '$'])
                .next()
                .unwrap()
                .trim_end_matches('/');
            let first = w.split('/').next().unwrap();
            (top.contains(first) && root.join(w).exists()).then(|| w.to_owned())
        })
        .collect()
}

/// `src` with only its test code kept: the item under each `#[cfg(test)]` that has a body. A
/// `mod x;` under one stands for a file of its own, which `test_only` picks up.
fn test_code(src: &str) -> String {
    let (code, _) = lex(src);
    let mut out = String::new();
    let mut from = 0;
    while let Some(at) = code[from..].find("#[cfg(test)]").map(|a| a + from) {
        let item = &code[at..];
        let semi = item.find(';').unwrap_or(item.len());
        match item.find('{').filter(|&o| o < semi) {
            Some(open) => {
                let end = block_end(&code, at + open);
                out.push_str(&src[at..end]);
                out.push('\n');
                from = end;
            }
            None => from = at + 1,
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Whether `file` under `cli/src/` holds only test code: a `tests.rs`, `testutil.rs` or file
/// under a `tests/` directory.
fn test_only(file: &Path) -> bool {
    let name = file.file_name().unwrap().to_string_lossy();
    name == "tests.rs"
        || name == "testutil.rs"
        || file.components().any(|c| c.as_os_str() == "tests")
}

/// Each cargo test target (`--test <name>`, `--bin ns`) with the repo paths its test sources
/// name: a `--test`'s files under `cli/tests/`, with `common/`, and `--bin ns`'s test code in
/// `cli/src/`. An `include_str!` or `include_bytes!` in `cli/src/` is compiled into `ns`, which
/// every target runs, so the paths they name count under `all`.
fn reads() -> BTreeMap<String, BTreeSet<String>> {
    let top = top_level();
    let cli = checkout().join("cli");
    let tests = cli.join("tests");
    let read = |files: &[PathBuf]| -> Vec<String> {
        files
            .iter()
            .map(|f| fs::read_to_string(f).unwrap())
            .collect()
    };
    let mut common = Vec::new();
    rust_files(&tests.join("common"), &mut common);
    let mut reads = BTreeMap::new();
    for e in fs::read_dir(&tests).unwrap() {
        let p = e.unwrap().path();
        let mut files = common.clone();
        let name = if p.extension().is_some_and(|x| x == "rs") {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            files.push(p.clone());
            if tests.join(&name).is_dir() {
                rust_files(&tests.join(&name), &mut files);
            }
            name
        } else if p.join("main.rs").is_file() {
            rust_files(&p, &mut files);
            p.file_name().unwrap().to_string_lossy().into_owned()
        } else {
            continue;
        };
        let literals = checkout_literals(&read(&files));
        reads.insert(format!("--test {name}"), repo_paths(&literals, &top));
    }
    let mut src = Vec::new();
    rust_files(&cli.join("src"), &mut src);
    let srcs = read(&src);
    let test: Vec<String> = src
        .iter()
        .zip(&srcs)
        .map(|(f, text)| {
            if test_only(f.strip_prefix(&cli).unwrap()) {
                text.clone()
            } else {
                test_code(text)
            }
        })
        .collect();
    reads.insert(
        "--bin ns".to_owned(),
        repo_paths(&checkout_literals(&test), &top),
    );
    let compiled: Vec<String> = srcs
        .iter()
        .flat_map(|text| {
            ["include_str!(", "include_bytes!("]
                .iter()
                .flat_map(move |m| text.split(m).skip(1))
                .filter_map(|s| s.split(')').next())
                .map(|s| s.trim().trim_matches('"').to_owned())
        })
        .collect();
    reads.insert("all".to_owned(), repo_paths(&compiled, &top));
    reads
}

/// The reads in `reads` that no entry of `map` lists for their target, one line each.
fn unmapped(map: &str, reads: &BTreeMap<String, BTreeSet<String>>) -> Vec<String> {
    let entries = entries(map);
    let mut missing = Vec::new();
    for (target, paths) in reads {
        for path in paths {
            let mapped = entries.iter().any(|(pattern, words)| {
                covers(pattern, path) && (words == &["all"] || targets(words).contains(target))
            });
            if !mapped {
                missing.push(format!("{target} reads {path}"));
            }
        }
    }
    missing
}

fn map() -> String {
    fs::read_to_string(checkout().join("scripts/ci-affected.map")).unwrap()
}

#[test]
fn every_repo_path_a_test_binary_names_is_mapped_to_it() {
    let reads = reads();
    assert!(reads["--bin ns"].contains("docs"), "{reads:?}");
    assert!(
        reads["all"].contains(".nightshift/phase-settings.json"),
        "{reads:?}"
    );
    let missing = unmapped(&map(), &reads);
    assert!(
        missing.is_empty(),
        "scripts/ci-affected.map lists no pattern for:\n{}",
        missing.join("\n")
    );
}

#[test]
fn a_stray_read_of_an_unmapped_path_fails_the_map_check() {
    let src = r##"
fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}
#[test]
fn reads_the_standards() {
    fs::read_to_string(checkout().join("CODING_STANDARDS.md")).unwrap();
}
"##;
    let mut reads = BTreeMap::new();
    let paths = repo_paths(&checkout_literals(&[src.to_owned()]), &top_level());
    reads.insert("--test cli".to_owned(), paths);
    assert_eq!(
        unmapped(&map(), &reads),
        ["--test cli reads CODING_STANDARDS.md"]
    );
    assert_eq!(
        unmapped("CODING_STANDARDS.md --test cli\n", &reads),
        [] as [&str; 0]
    );
    assert_eq!(unmapped("CODING_* all\n", &reads), [] as [&str; 0]);
    assert_eq!(
        unmapped("CODING_STANDARDS.md --test eval\n", &reads).len(),
        1
    );
}

#[test]
fn only_literals_in_code_that_reaches_the_checkout_count_as_reads() {
    let src = r##"
const FROM_CLI: &str = "../scripts/stress.sh";
// "README.md" in a comment
fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}
fn root() -> Repo {
    let _ = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/{x}");
    Repo::new()
}
fn via_path_fn() {
    let _ = (checkout().join("skills/ns-tdd"), '"', '\'', "rules", r#"x "LICENSE" y"#);
}
fn via_other_fn() {
    let _ = (root(), "NOTICE.md");
}
fn in_a_temp_repo() {
    let _ = ("CARGO_MANIFEST_DIR", "AGENTS.md", "skills/no-such-skill");
}
"##;
    let paths = repo_paths(&checkout_literals(&[src.to_owned()]), &top_level());
    assert_eq!(
        paths.into_iter().collect::<Vec<_>>(),
        [
            "LICENSE",
            "docs",
            "rules",
            "scripts/stress.sh",
            "skills/ns-tdd"
        ]
    );
}

#[test]
fn test_code_is_the_items_under_cfg_test() {
    let src = "fn live() { \"README.md\" }\n#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod t {\n    fn f() { \"{\" }\n}\nfn after() {}\n";
    assert_eq!(
        test_code(src),
        "#[cfg(test)]\nmod t {\n    fn f() { \"{\" }\n}\n"
    );
}

#[test]
fn every_map_target_is_a_test_binary_and_the_scripts_that_run_tests_run_all() {
    let tests = checkout().join("cli/tests");
    for (pattern, words) in entries(&map()) {
        for t in targets(&words) {
            let ok = match t.split_once(' ') {
                _ if t == "all" => true,
                Some(("--bin", name)) => name == "ns",
                Some(("--test", name)) => {
                    tests.join(format!("{name}.rs")).is_file()
                        || tests.join(name).join("main.rs").is_file()
                }
                _ => false,
            };
            assert!(
                ok,
                "scripts/ci-affected.map: {pattern}: {t} is no test binary"
            );
        }
    }
    let all: Vec<String> = entries(&map())
        .into_iter()
        .filter(|(_, w)| w == &["all"])
        .map(|(p, _)| p)
        .collect();
    for p in [
        "cli/*",
        "Cargo.lock",
        "scripts/ci-local.sh",
        "scripts/ci-affected.sh",
        "scripts/ci-affected.map",
        "scripts/ci-tools.d/*",
    ] {
        assert!(all.iter().any(|a| a == p), "{p} must run all of cargo test");
    }
}
