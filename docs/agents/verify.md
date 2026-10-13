# Verify

How to prove a change works on the real surface, beyond the test suite. `ns-verify` reads this file.

## Set up

- **Verify skill**: `verify-nightshift` at `.agents/skills/verify-nightshift/SKILL.md`, feature map in `features/`. Load it to drive the product and collect evidence.
- **Control CLI**: `ns` itself, built from the worktree (`cli/target/debug/ns`), run against a throwaway git repo with the fake `claude`, `gh` and `glab` from `cli/tests/fixtures/factory/`. There is no separate control script. Start with the skill's Doctor, then `cli/target/debug/ns --help`.
- **Hooks**: a worktree from `ns worktree new` sets `core.hooksPath` to `.githooks`, whose hooks run `scripts/ci-local.sh`. Pre-commit runs its `--fast` subset (pinned tools, fmt, clippy, the `ns` build, `ns lint`, `ns check-markers`, the private guard) on the working tree, in about a minute. Pre-push runs the same `--fast` subset in a temporary worktree at each commit pushed, whatever the working tree holds, because `.nightshift/nightshift.toml` sets `[gate] local = "build-only"`: `ns run`'s gate after build runs all of CI, and GitHub CI runs it before the merge (ADR-466). A full run that passes records its tree in `<git-common-dir>/ns/ci-passed/`, so pre-push and the `ns run` gates skip a tree that already passed, printing `ci-local: tree <hash> already passed (<when>, <where>)`; `scripts/ci-local.sh --no-cache` runs anyway. Its first step fails on a tool not pinned in `scripts/ci-tools.d/`, the pins CI installs from, or at another version, and names the command that installs it.
- **CPU share**: a gate or verify script that starts parallel work of its own reads `NS_CPUS`, the unit's share of the cores under `ns watch --parallel` (`docs/factory/watch.md`, "Parallel units"). `cargo build` and `cargo test` need nothing: `ns run --cpus` also sets `CARGO_BUILD_JOBS` and `RUST_TEST_THREADS`, which cargo reads, so `scripts/ci-local.sh` passes no `-j` of its own.
- **Evidence**: `.ns/verify/<run-id>/` by default. Export `OUT=<dir>` before the skill's Launch to override.
- **Surfaces**: covered: the `ns` CLI, and the skills through `ns lint` and `ns eval --dry-run`. Not covered: real harness runs and real GitHub or GitLab calls, which verification never makes; skill behaviour under a model, which scheduled `ns eval` runs measure (`docs/EVALS.md`).
- **Last proven end to end**: 2026-10-11, commit 607a754, feature `worktree` (`wt-new`, `wt-idempotent`, `wt-invalid`, `wt-setup`, `wt-remove`).
