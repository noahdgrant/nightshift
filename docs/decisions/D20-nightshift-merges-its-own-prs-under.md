---
id: D20
title: "nightshift merges its own PRs under merge.policy = auto"
date: 2026-10-08
---

# D20: nightshift merges its own PRs under merge.policy = auto

## Decision

nightshift merges its own PRs when review passes, CI is green and no file that needs human review changed (`merge.policy = auto`). Files that need human review cover the factory's own guardrails and still need a human merge.

## Reason

Only `ns run`'s merge step merges, only its own unit's PR, only by squash. A PR merged by a phase stops the run. Spec: `docs/factory/merge.md`.
