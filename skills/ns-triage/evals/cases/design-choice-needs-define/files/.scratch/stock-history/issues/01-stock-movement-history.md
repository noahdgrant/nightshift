# Keep a history of stock movements

Status: needs-triage
Category:
Priority: priority:medium

Reported by: Sam (purchasing)

## Description

When the on-hand figure for an item looks wrong, nobody can tell how it got there. Record every receive and fulfil, and add `inventory history <sku>`, which prints that item's movements oldest first, one line each: the ISO timestamp, `receive` or `fulfil`, and the quantity.

## Acceptance criteria

- [ ] Pick the storage mechanism for the history and note why: a `movements` list inside the existing state file, a separate JSON Lines file next to it, or a SQLite database.
- [ ] `receive` and `fulfil` each record one movement at the command's `--now` time.
- [ ] `inventory history BOLT-M6` prints BOLT-M6's movements, oldest first.
- [ ] State files written before this change still load.

## Comments
