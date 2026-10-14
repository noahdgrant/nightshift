//! The core does no I/O: no file in `run/core` or `watch/core` (nor `run/core.rs` and
//! `watch/core.rs`) names a module or call that reads a file, runs a process, opens a socket,
//! reads the environment or the clock, or logs. A new file there is checked without being listed.
//! Test-only files (`tests.rs`, `sim.rs`, this file, `scenarios/`) and inline `#[cfg(test)]`
//! modules are left out: tests may do I/O.

use std::fs;
use std::path::{Path, PathBuf};

/// What a core file must not name.
const DENIED: &[&str] = &[
    "std::fs",
    "std::process",
    "std::net",
    "std::env",
    "std::io",
    "std::thread",
    "SystemTime",
    "Instant",
    "crate::proc",
    "Proc",
    "crate::git",
    "git::",
    "gh(",
    "gh_json",
    "Clock",
    "eprintln!",
    "println!",
    "log_event",
];

/// The files the core is made of.
fn core_files() -> Vec<PathBuf> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = vec![];
    for core in ["run/core", "watch/core"] {
        out.push(src.join(format!("{core}.rs")));
        walk(&src.join(core), &mut out);
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "scenarios" {
                walk(&p, out);
            }
        } else if name.ends_with(".rs") && !["tests.rs", "sim.rs", "purity.rs"].contains(&&*name) {
            out.push(p);
        }
    }
}

fn ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `pattern` occurs in `text` as a whole name: not inside a longer identifier.
fn names(text: &str, pattern: &str) -> bool {
    text.match_indices(pattern).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + pattern.len()..].chars().next();
        let starts = pattern.starts_with(ident);
        let ends = pattern.ends_with(ident);
        let glued = (starts && before.is_some_and(ident)) || (ends && after.is_some_and(ident));
        !glued
    })
}

/// Every path a `use` tree names, with its braces expanded: `std::{fs, path::Path}` gives
/// `std::fs` and `std::path::Path`.
fn expand(tree: &str) -> Vec<String> {
    let Some(open) = tree.find('{') else {
        return vec![tree.to_string()];
    };
    let (prefix, inner) = (
        &tree[..open],
        &tree[open + 1..tree.rfind('}').unwrap_or(tree.len())],
    );
    let mut parts = vec![];
    let (mut depth, mut start) = (0, 0);
    for (i, c) in inner.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .flat_map(|p| expand(&format!("{prefix}{p}")))
        .collect()
}

/// The code a file holds, with comments and inline `#[cfg(test)]` modules dropped and each `use`
/// statement joined onto one line, braces expanded.
fn code(text: &str) -> Vec<String> {
    let mut out = vec![];
    let (mut test_attr, mut skipping, mut using) = (false, false, None::<String>);
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if skipping {
            skipping = line != "}";
            continue;
        }
        if test_attr && line.starts_with("mod ") && line.ends_with('{') {
            skipping = true;
            continue;
        }
        test_attr = line == "#[cfg(test)]";
        if let Some(u) = using.as_mut() {
            u.push_str(line);
        } else if line.starts_with("use ") || line.starts_with("pub use ") {
            using = Some(line.to_string());
        } else {
            out.push(line.to_string());
            continue;
        }
        if let Some(u) = using.take_if(|u| u.ends_with(';')) {
            let tree = u
                .trim_end_matches(';')
                .split_once("use ")
                .map_or("", |x| x.1);
            out.extend(expand(&tree.replace(' ', "")));
        }
    }
    out
}

/// The denied names `text` holds, with the line that holds each.
fn breaches(text: &str) -> Vec<(String, &'static str)> {
    code(text)
        .into_iter()
        .flat_map(|l| {
            DENIED
                .iter()
                .filter(|d| names(&l, d))
                .map(|d| (l.clone(), *d))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn the_core_does_no_io() {
    let files = core_files();
    for want in [
        "run/core.rs",
        "run/core/route.rs",
        "run/core/step.rs",
        "watch/core.rs",
    ] {
        assert!(
            files.iter().any(|f| f.ends_with(want)),
            "{want} is not checked: {files:?}"
        );
    }
    let mut found = vec![];
    for f in &files {
        let text = fs::read_to_string(f).unwrap();
        for (line, name) in breaches(&text) {
            found.push(format!("{}: `{name}` in `{line}`", f.display()));
        }
    }
    assert!(found.is_empty(), "the core does I/O:\n{}", found.join("\n"));
}

#[test]
fn the_check_sees_grouped_imports_and_whole_names_only() {
    let hits = |text: &str| -> Vec<&str> { breaches(text).into_iter().map(|b| b.1).collect() };
    assert_eq!(
        hits("use std::{\n    collections::BTreeMap,\n    fs,\n};\n"),
        ["std::fs"]
    );
    assert_eq!(hits("use std::process::Command;\n"), ["std::process"]);
    assert_eq!(
        hits("let n = crate::git::same_sha(a, b);\n"),
        ["crate::git", "git::"]
    );
    assert_eq!(hits("eprintln!(\"x\");\n"), ["eprintln!"]);
    // Whole names only, and comments and inline test modules don't count.
    assert!(hits("let high(x) = Processed;\nlet gate::Trigger = t;\n").is_empty());
    assert!(hits("// reads `git::diff_id`\nlet a = 1; // std::fs\n").is_empty());
    assert!(hits("#[cfg(test)]\nmod tests {\n    use std::fs;\n}\nfn f() {}\n").is_empty());
}
