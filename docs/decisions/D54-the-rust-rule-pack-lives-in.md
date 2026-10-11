---
id: D54
title: "The Rust rule pack lives in rules/rust/"
issue: 235
date: 2026-10-10
amended_by: [D58]
---

# D54: The Rust rule pack lives in rules/rust/

## Decision

Builds on D50 (#235). The Rust rule pack lives in `rules/rust/`: `check.sh <tool> <crate-dir>` runs one tool (clippy, deny or ast-grep) and prints its findings as D50 JSON lines with `jq`; `install.sh <repo> [<crate-dir>]` copies the pack into a target's `.nightshift/rules/rust/` and writes its `rules` row, and `ns-setup` runs it for a repo with a `Cargo.toml`. Four rules: clippy with `-D warnings` plus the pedantic lints in `clippy-lints.txt`; cargo-deny bans (a crate outside `allowed-crates.txt`, duplicate versions) and licences, with every diagnostic it prints, warning or error, a finding, as clippy's are under `-D warnings`; ast-grep `suppression-without-reason` (an `#[allow]` or `#[expect]` with no `reason =`), `file-too-long` (past 1000 lines) and `duplicate-helper` (a function or type named like a helper in `roads.txt`, outside that helper's file). nightshift runs the pack from `rules/rust/` on `cli/`, with its own lists there.

## Reason

One script per pack, not per tool, keeps the tool checks, temp files and the fail-on-no-findings rule in one place; its `jq` filters are the only conversion, since none of the three prints D50's line formats with a rule id. `jq` over Python because a Rust repo is more likely to have it, and CI runners ship it. The allowlist and roads are plain lists the repo owns, kept apart from the pack files so a rerun of `install.sh` refreshes the pack without touching them; the install writes the allowlist from `cargo metadata`, so it starts as today's graph. cargo-deny's `bans.allow` covers the whole graph, so a new crate pulled in by a dependency counts too, and an empty list (which cargo-deny reads as "allow all") is an error. `reason =` inside the attribute is the reason on the same line: a trailing comment is a sibling node ast-grep cannot tie to the line. `duplicate-helper` matches by name only, since a duplicate's shape is not something a pattern can compare across files; the roads list is the repo's, filled during setup. The pack's tests run under `cargo test` and fail naming any of `cargo`, `jq`, `ast-grep` or `cargo-deny` that is missing; CI installs the last two with `taiki-e/install-action`. The `ns-setup` rules step has no eval case: it runs one deterministic script (`install.sh`), which `cli/tests/rules_pack.rs` tests end to end, and no eval fixture is a Rust repo.
