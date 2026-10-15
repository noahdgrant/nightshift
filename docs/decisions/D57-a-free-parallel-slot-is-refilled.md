---
id: D57
title: "A free parallel slot is refilled every 60 seconds"
issue: 275
date: 2026-10-10
amends: [D46]
amended_by: [ADR-641]
---

# D57: A free parallel slot is refilled every 60 seconds

## Decision

Amends D46. A slot left free beside running units is filled again every 60 seconds (`REFILL`; `NS_REFILL_MS` overrides it for tests), not only when a unit ends: the scheduler's wait wakes, and the same fill runs, triage pass (D42 order and bounds) included, but not the between-units worktree cleanup or the update check (D49), which run at start and when a unit ends. The wake is off while the night is stopping, draining (D49) or paused. A refill's listing, claim or start error is logged and the next interval tries again; a hold that cannot be written on a pause is fatal on any fill.

## Reason

A slot beside a long unit stayed idle all night while the backlog held issues that triage would have made ready, since the fill, the only caller of the triage pass, ran only at start and when a unit ended (#275). Reusing the whole fill keeps D46's claim-before-start and taken-tonight rules on one path. An interval rather than a trigger, since a new candidate shows only in a listing. A re-fill with nothing to do lists candidates and the queue once each and runs no triage, since each candidate is tried once a night; it makes no per-worktree `gh` call and no `git fetch`, so 60 seconds keeps a long unit's free slot responsive at about two `gh` listings a minute. `NS_REFILL_MS` is held to at least the 50 ms poll. Real time, not the night's clock, so a pinned clock (`NS_NOW`) still refills. Spec: docs/factory/watch.md ("Parallel units").
