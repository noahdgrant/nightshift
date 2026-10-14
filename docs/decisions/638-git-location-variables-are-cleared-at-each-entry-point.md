---
id: ADR-638
title: "Git location variables are cleared at each entry point, not caught by a decoy GIT_DIR run"
issue: 638
date: 2026-10-14
amends: [ADR-367, ADR-420, D55]
---

# ADR-638: Git location variables are cleared at each entry point, not caught by a decoy GIT_DIR run

## Decision

Three entry points keep `GIT_DIR` and the other variables in `git::LOCATION_VARS` from reaching a git process:

1. `ns` removes them from its own environment at the top of `main`, before any thread starts. `proc` still removes them from each child it starts.
2. The shared test helpers that run git or start a child (`common::command` in `cli/tests/common/mod.rs`, `testutil::g` in `cli/src/testutil.rs`) panic, naming each variable set, when the test process holds any of them, empty or not. A test that aims one child at a decoy sets the variable on that child only.
3. `scripts/ci-local.sh`, each hook in `.githooks/` and each `rules/*/tests/run.sh` unset them as their first command, after only `set` and `shopt` option lines.

`cli/tests/git_location.rs` tests each one. CI and `scripts/ci-local.sh` no longer run the cargo suite or the rule pack tests a second time under a decoy `GIT_DIR`, and `scripts/check-git-dir-decoy.sh` is gone. The #120 regression tests that aim `ns`, a pack's scripts or `scripts/check-verify-recipes.sh` at a decoy repo stay in the normal suite as the canary.

## Reason

The decoy run (#120) caught a leak only after the fact and doubled the time of the slowest CI steps: the whole cargo suite and every rule pack ran twice. Clearing the variables where a process enters the repo's code closes the leak at its source, and the helper assertion stops a test from running in a leaky environment before it touches a repo. The human agreed the design in #638 on 2026-10-14.
