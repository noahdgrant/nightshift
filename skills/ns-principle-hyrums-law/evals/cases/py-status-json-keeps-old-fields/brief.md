---
unit: 11-status-json-grouped
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** `inventory status --json` prints bare numbers next to the SKU, with no item name. Give each entry its name, and group the three counts in one `stock` object so a reader sees them as one figure.

**Acceptance criteria:**
- [ ] Each entry in `inventory status --json` has the item's `name`.
- [ ] Each entry's counts sit in a `stock` object: `{"on_hand": 10, "reserved": 4, "available": 6}`.
- [ ] `inventory status` without `--json` prints the same table as today.
- [ ] Existing tests still pass.

**Out of scope:** the table output, the state file format.
