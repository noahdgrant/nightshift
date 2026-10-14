---
unit: 6-status-json
phase: triage
status: pass
base: main
---
## Agent Brief

**Category:** enhancement

**Summary:** Add `--json` to `inventory status`, so scripts can read stock levels without parsing the table.

**Acceptance criteria:**
- [ ] `inventory status --json` prints a JSON array with one object per item, in SKU order. Each object has the keys `sku`, `on_hand`, `reserved` and `available`, with the same figures the table shows.
- [ ] `inventory status --json <SKU>` prints an array holding only that item's object.
- [ ] `inventory status --json <SKU>` for an unknown SKU prints nothing on stdout, prints `inventory: error: unknown item '<SKU>'` on stderr, and exits 1.
- [ ] The dashboard team's parser accepts the output of `inventory status --json` for a store of 100000 items.

**Out of scope:** the table output, expiry rules, the state file format.
