---
id: ADR-452
title: "This repo's pre-commit hook runs the fast CI subset; pre-push runs all of CI"
issue: 452
date: 2026-10-12
amends: [D64]
amended_by: [ADR-466]
---

# ADR-452: This repo's pre-commit hook runs the fast CI subset; pre-push runs all of CI

## Decision

`.githooks/pre-commit` runs `scripts/ci-local.sh --fast` on the working tree (#452, part A of #444). `--fast` runs these steps of the full run, in its order and with the same names and commands: the pinned tools, `cargo fmt --check`, clippy, the `ns` build, `ns lint` of `skills` and `.agents/skills`, `ns check-markers` and the private guard. `.githooks/pre-push` is unchanged (D67): it runs all of `ci-local.sh` in a temporary worktree at each commit pushed. `ci-local.sh` with no argument still runs all of CI, and any other argument is a usage error. The rest of D64 stands: agents never skip a hook.

## Reason

D64 dropped `--fast` because a commit could pass it and still fail CI. Since D67, pre-push runs all of CI on exactly the commit pushed, in a clean checkout, so nothing broken leaves the machine and a full pre-commit adds no guarantee. It did add cost: a factory unit ran the full suite about six times, mostly on the same tree, with a full run on every commit. On 2026-10-12 three runs at once on one laptop made the factory crawl and turned load into flaky test failures (#443, #448, #449). The human agreed the change on 2026-10-12. Pre-commit keeps the checks that catch most mistakes in about a minute.
