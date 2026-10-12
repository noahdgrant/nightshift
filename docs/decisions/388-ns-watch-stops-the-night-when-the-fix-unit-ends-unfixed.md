---
id: ADR-388
title: "ns watch stops the night when the fix unit ends unfixed"
issue: 388
date: 2026-10-11
amends: [ADR-387]
---

# ADR-388: ns watch stops the night when the fix unit ends unfixed

## Decision

When the fix issue's unit ends `stuck`, `budget` or `split` and main is still red, `ns watch` logs `main_red_unfixed` (`sha`, `issue`, `outcome`) and stops the night as `--until` does: nothing new starts, running units finish, paused units go back to the queue, and the summary says `stopped: "main_red_unfixed"`. No second fix attempt runs. `ns status` shows `main_red`, `main_green` and `main_red_unfixed` in a `main` part, and `ns-supervise` pings on `main_red` and `main_red_unfixed`.

## Reason

Agreed in #377. A fix unit that ends unfixed leaves main red and the queue held, so the night would idle until `--until`, and a second attempt would fail the same way. Main's CI is read again as the unit ends: a main that a person fixed meanwhile, or whose state can't be read, stops nothing, as a CI read that fails holds nothing (ADR-387). A fix unit the harness failed goes back to the queue and counts toward the harness breaker, which stops the night on its own. A `done` fix unit leaves a PR waiting on a human merge, which may still fix main, so the night goes on holding. The run log's `sha` field is now kept by `ns log`, so `main_red`, `main_green`, `main_red_unfixed`, and the `phase`, `gate`, `merged` and `merge_gate_failed` events that already logged one, show the head they name.
