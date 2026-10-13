# Rust rule pack

The rules `ns rules check` runs on a Rust repo (`docs/factory/run.md`, "Rules"). The pack composes clippy, cargo-deny and ast-grep; D54 in `docs/decisions/` lists the rules, and D58 the handling of compile errors. `check.sh <tool> <crate-dir>` runs one tool and prints each finding as a JSON line `ns` reads, and `install.sh <repo> [<crate-dir>]` copies the pack into a repo's `.nightshift/rules/rust/` and writes its `rules` row.

## Wall-clock waits in tests

The ast-grep rule `wall-clock-in-test` (#553) enforces `CODING_STANDARDS.md`, "A test never waits on the clock". It flags a call to any `sleep` (`std::thread::sleep`, an imported `sleep`, `tokio::time::sleep`), a `<`, `<=`, `>` or `>=` comparison with `Instant::now()` or `.elapsed()`, and a `Duration::from_*` with a literal argument. Inside a macro such as `assert!`, where ast-grep sees tokens rather than expressions, it flags a comparison next to `Instant::now` or any identifier named `elapsed`, a variable too.

It flags them only in test code: any file under a `tests/` directory, and any item marked `#[test]`, `#[cfg(test)]` or `#[<crate>::test]` (the attribute right before the item, or the one before that). Production code is never flagged. ast-grep has no rule that matches a path or a scope, so `ast-grep.yml` holds the rule twice, one copy per scope; keep their matchers the same.

A literal `Duration` that only builds a fake-clock value, or a generous hang guard, is flagged too. Baseline it with `ns rules baseline --allow-growth`, which takes a human's yes.

## Compile errors

A compile error is not a finding, so it can't be baselined. Cargo can't check a crate whose dependency didn't compile, so a baselined error would hide every finding behind it. When clippy reports a compiler error, an error with no code or an `E` code such as `E0308`, `check.sh clippy` exits 2 with no findings. It prints the errors, and the findings from the members it did lint (it runs with `--keep-going`), as `check.sh:` lines on stderr:

```text
check.sh: also found clippy::needless_pass_by_value at ./good/src/lib.rs:1
check.sh: . has compile errors, so clippy skipped every crate that depends on a failing one:
check.sh:   ./bad/src/lib.rs:1: error: this file contains an unclosed delimiter
```

`ns rules check` then fails as broken, and `ns rules baseline` records nothing, as with the C pack (D56). Fix the errors and rerun. Lints, clippy's and rustc's (`dead_code`), stay findings.
