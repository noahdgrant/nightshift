---
unit: 9-status-expired
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** `inventory status` still counts a reservation as reserved after it expires, so the stock looks held when it is free.

**Acceptance criteria:**
- [ ] `Warehouse.stock_levels(now)` counts only reservations that have not expired at `now`, so `status` shows their stock as available again.
- [ ] To keep `status` fast, `stock_levels(now)` deletes the reservations that have expired at `now` from the warehouse as it goes.
- [ ] Existing tests still pass.

**Out of scope:** the CLI's output format.
