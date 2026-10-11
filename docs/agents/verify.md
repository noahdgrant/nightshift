# Verify

How to prove a change works on the real surface, beyond the test suite. `ns-verify` reads this file.

## Set up

- **Verify skill**: `verify-nightshift` at `.agents/skills/verify-nightshift/SKILL.md`, feature map in `features/`. Load it to drive the product and collect evidence.
- **Control CLI**: `ns` itself, built from the worktree (`cli/target/debug/ns`), run against a throwaway git repo with the fake `claude`, `gh` and `glab` from `cli/tests/fixtures/factory/`. There is no separate control script. Start with the skill's Doctor, then `cli/target/debug/ns --help`.
- **Evidence**: `.ns/verify/<run-id>/` by default. Export `OUT=<dir>` before the skill's Launch to override.
- **Surfaces**: covered: the `ns` CLI, and the skills through `ns lint` and `ns eval --dry-run`. Not covered: real harness runs and real GitHub or GitLab calls, which verification never makes; skill behaviour under a model, which scheduled `ns eval` runs measure (`docs/EVALS.md`).
- **Last proven end to end**: 2026-10-11, commit 607a754, feature `worktree` (`wt-new`, `wt-idempotent`, `wt-invalid`, `wt-setup`, `wt-remove`).
