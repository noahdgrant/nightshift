---
unit: 7-reserve-exact
phase: triage
status: pass
base: main
---
## Agent Brief

**Category:** bug

**Summary:** `Warehouse.reserve` refuses a reservation for exactly the quantity available, so the last units of an item can never be reserved.

**Current behavior:** with 5 of `BOLT-M6` on hand and nothing reserved, reserving 5 raises `InsufficientStock` ("cannot reserve 5 of 'BOLT-M6': only 5 available"). Reserving 4 works, and reserving 6 is refused.

**Repro:**

```bash
S=$(mktemp -d)/inv.json
inv() { PYTHONPATH=src python3 -m inventory --state "$S" --now 2026-04-01T10:00:00 "$@"; }
inv add-item BOLT-M6 "M6 hex bolt"
inv receive BOLT-M6 5
inv reserve BOLT-M6 5
# inventory: error: cannot reserve 5 of 'BOLT-M6': only 5 available   (exit 1)
```

**Acceptance criteria:**
- [ ] Reserving exactly the quantity available at `now` succeeds and leaves nothing available.
- [ ] Reserving the remainder after other reservations succeeds (7 on hand, 3 reserved, reserve 4).
- [ ] Reserving more than is available still raises `InsufficientStock`.

**Out of scope:** expiry rules, `stock_levels`, the CLI output format.
