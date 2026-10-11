---
id: D23
title: "Runners name exclusive locks held for the whole phase"
date: 2026-10-09
amended_by: [D33]
---

# D23: Runners name exclusive locks held for the whole phase

## Decision

A runner (`runners/<name>.toml`) names exclusive locks, and a phase that sets `runner =` holds them for the whole phase. A run that finds a lock held waits for it; it doesn't refuse.

## Reason

Overnight, a bench or Zephyr workspace shared by two repos' `ns watch` should queue the second unit, not end its night with exit 5. The locks are OS `flock`s, so a dead run's hold goes with its process. Taken in sorted order, so two runs can't deadlock. Amended by D33. Spec: `docs/FACTORY.md`.
