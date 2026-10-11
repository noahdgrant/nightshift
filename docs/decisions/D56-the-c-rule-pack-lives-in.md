---
id: D56
title: "The C rule pack lives in rules/c/"
issue: 236
date: 2026-10-10
---

# D56: The C rule pack lives in rules/c/

## Decision

Builds on D50. The C rule pack (#236) lives in nightshift under `rules/c/`; `ns-setup` copies it into a target repo's `.nightshift/rules/c/`, so every pack installs under `.nightshift/rules/<language>/` (#283). It is one stdlib-only Python wrapper, `rules-c`, with a subcommand per tool (`grep` for ast-grep, `tidy` for clang-tidy with `-Wall -Wextra`, `size` for the 1000-line limit) that prints D50's JSON lines and exits 2 with no findings when it can't run. The include allowlist is `.nightshift/rules-c-includes.txt`. Each rule has a failing and a passing case under `rules/c/tests/cases/<rule>/`, run by the `rule-pack-c` CI job and `scripts/ci-local.sh`.

## Reason

The pack sits in `.nightshift/` for the reason D55 gives: that path needs a human merge, so an agent can't loosen a rule to get its unit through. It first installed at `rules/c/`, the same path as in nightshift, to keep the rows copyable; #283 moved it to match the Rust and Python packs (D54, D55). Python because every Zephyr repo already needs it and a shell wrapper would need `jq`. ast-grep finds the shapes (definitions, includes, suppressions); the wrapper does the cross-file part ast-grep can't (a name defined in two files, a header outside the allowlist). Duplicate names under `test/` and `tests/` are skipped: each Zephyr test suite is its own program with its own `before` hook. Code clang can't compile is an error rather than a finding, because a baselined compile error would hide every finding behind it; so `tidy` needs the build's `compile_commands.json`. Every `error:` diagnostic counts, not only `clang-diagnostic-error`: an implicit declaration is an error under its own warning name. `tidy` adds `-Wno-error` so a build's blanket `-Werror` doesn't turn its warnings into compile errors. The allowlist sits under `.nightshift/` because adding a dependency is a human decision. The CI job edits `.github/**` because ast-grep has to be installed; `cargo test` can't run the cases without it.
