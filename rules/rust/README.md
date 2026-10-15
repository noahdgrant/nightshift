# Rust rule pack

The rules `ns rules check` runs on a Rust repo (`docs/factory/run.md`, "Rules"). The pack composes clippy, cargo-deny and ast-grep; D54 in `docs/decisions/` lists the rules, and D58 the handling of compile errors. `check.sh <tool> <crate-dir>` runs one tool and prints each finding as a JSON line `ns` reads, and `install.sh <repo> [<crate-dir>]` copies the pack into a repo's `.nightshift/rules/rust/` and writes its `rules` row.

## Wall-clock waits in tests

The ast-grep rule `wall-clock-in-test` (#553) enforces `CODING_STANDARDS.md`, "A test never waits on the clock". It flags a call to any `sleep` (`std::thread::sleep`, an imported `sleep`, `tokio::time::sleep`), a `<`, `<=`, `>` or `>=` comparison with `Instant::now()` or `.elapsed()`, and a `Duration::from_*` with a literal argument. Inside a macro such as `assert!`, where ast-grep sees tokens rather than expressions, it flags a comparison next to `Instant::now` or any identifier named `elapsed`, a variable too.

It flags them only in test code: any file under a `tests/` directory or named `tests.rs`, and any item marked `#[test]`, `#[cfg(test)]` or `#[<crate>::test]` (among the attributes and comments right before the item, however many). Production code is never flagged. ast-grep has no rule that matches a path or a scope, so `ast-grep.yml` holds the rule twice, one copy per scope. A pack test asserts the two copies match.

A literal `Duration` that only builds a fake-clock value, or a generous hang guard, is flagged too. Baseline it with `ns rules baseline --allow-growth`, which takes a human's yes.

## Harness processes built outside crate::harness

The ast-grep rule `harness-kind-construction` (#716) closes the gap the clippy ban on `Proc::harness` (`cli/clippy.toml`) leaves. A `Proc` of kind `Kind::Harness` can also be built through `memcap::command`, `memcap::wrapped` or `Proc::launched`, which skip the billing and phase setup `crate::harness::command` adds (#657). The rule flags any `Kind::Harness { .. }` built as a value, with or without a path before `Kind` (`proc::Kind::Harness { subscription }`), in every file except the three allowed to build one: `cli/src/harness.rs`, `cli/src/proc.rs` (`Proc::harness`, behind the clippy ban) and `cli/src/proc/tests.rs`. Its `ignores:` globs name those paths from the repo root, where `ns rules check` runs `check.sh`. A pattern that matches the variant (a `match` arm, `if let Kind::Harness { .. } = k`) is not flagged.

It misses three cases, each rare enough to leave to review:

- Inside a macro, ast-grep sees tokens rather than an expression, and can't tell a value from a pattern (`matches!(k, Kind::Harness { .. })`), so the rule flags neither. Build the value outside the macro in production code.
- A name other than `Kind::Harness` for the variant, from `use Kind::*`, `use proc::Kind as K` or `Self::Harness` inside `impl Kind`.
- A `Kind` copied or passed in from elsewhere, which builds no new value.

`rules/rust/tests/run.sh` checks the flagged and unflagged forms and each allowed file.

## Compile errors

A compile error is not a finding, so it can't be baselined. Cargo can't check a crate whose dependency didn't compile, so a baselined error would hide every finding behind it. When clippy reports a compiler error, an error with no code or an `E` code such as `E0308`, `check.sh clippy` exits 2 with no findings. It prints the errors, and the findings from the members it did lint (it runs with `--keep-going`), as `check.sh:` lines on stderr:

```text
check.sh: also found clippy::needless_pass_by_value at ./good/src/lib.rs:1
check.sh: . has compile errors, so clippy skipped every crate that depends on a failing one:
check.sh:   ./bad/src/lib.rs:1: error: this file contains an unclosed delimiter
```

`ns rules check` then fails as broken, and `ns rules baseline` records nothing, as with the C pack (D56). Fix the errors and rerun. Lints, clippy's and rustc's (`dead_code`), stay findings.

## Binary size ratchet

`ratchet-binary-size.sh <bin> [<crate-dir>]` is an example ratchet (`docs/factory/run.md`, "Ratchets"): it builds the binary with the release profile and prints its size in bytes as the one line on stdout, so `ns ratchet check` fails a unit that grows it. It reads the executable's path from cargo's JSON messages, so a workspace or a `CARGO_TARGET_DIR` finds the right file. A failed build, an unknown binary or a missing `cargo` or `jq` makes it exit 1 with a message and print nothing. `install.sh` copies it, and `ratchet.toml` is its `[ratchets.binary-size]` table, which `ns-setup` offers to add to `.nightshift/nightshift.toml` with `{bin}` and `{crate}` filled in. `tests/run.sh` builds a small workspace and checks the script and the table; CI and `scripts/ci-local.sh` run it.
