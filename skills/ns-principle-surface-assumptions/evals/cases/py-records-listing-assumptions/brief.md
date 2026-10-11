---
unit: 6-list-reservations
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** List the reservations held against an item from the command line: `inventory reservations [SKU]`.

**Acceptance criteria:**
- [ ] `inventory reservations BOLT-M6` prints one line per reservation, in the format `reserve` prints: `R0001: 2 x BOLT-M6 until 2026-03-02T09:15:00`.
- [ ] Without a SKU, it lists the reservations for every item.
- [ ] An unknown SKU exits non-zero with an error.

**Out of scope:** changing how `reserve` prints a reservation.
