# Keep a history of stock movements

Status: needs-triage
Category:
Priority: priority:medium

Reported by: Sam (purchasing)

## Description

When the on-hand figure for an item looks wrong, nobody can tell how it got there. Record every receive and fulfil, and add `inventory history <sku>`, which prints that item's movements oldest first, one line each: the ISO timestamp, `receive` or `fulfil`, and the quantity.

I've decided the history lives in the state file itself, so there is no second file to back up or keep in step.

## Acceptance criteria

- [ ] Store the history as a `movements` list on each item inside the existing state file. A state file without it loads with an empty history, so the state version stays 1.
- [ ] `receive` and `fulfil` each record one movement at the command's `--now` time.
- [ ] `inventory history BOLT-M6` prints BOLT-M6's movements, oldest first.
- [ ] State files written before this change still load.

## Comments
