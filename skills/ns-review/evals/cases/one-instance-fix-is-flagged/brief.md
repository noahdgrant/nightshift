---
unit: 4-expired-holds
phase: triage
status: pass
base: main
---
## Agent Brief

**Category:** bug

**Summary:** Reports count reservations that have expired, so they show stock as held after the hold ends.

**Current behavior:** `Warehouse.stock_levels(now)` sums every reservation per SKU without checking expiry, so `StockLevel.reserved` and `.available` (and the CLI `status`) still include a reservation after it expires. `Warehouse.reserved()` and `available()` go through `_active()` and are correct.

**Desired behavior:** every report counts only the reservations still held at `now`, through the same active-reservation logic `reserved()` uses.

**Instances:**
- Search: `grep -n "_reservations.values()" src/` for every loop over the held reservations, then read each hit for an expiry check
- `Warehouse.stock_levels()`: the reported instance, covered
- `Warehouse.low_stock()`: the same unfiltered sum, so an item whose holds have expired stays listed as low, covered
- `Warehouse._active()` and `reservations()`: no defect (`_active` filters expiry; `reservations()` lists expired holds by design)

**Failure modes:** none; both methods are pure reads of in-memory state.

**Edge cases:**
- A reservation at exactly its `expires_at` no longer counts (`Reservation.is_expired` is `now >= expires_at`)
- An item with no reservations reports 0 reserved

**Acceptance criteria:**
- [ ] `stock_levels(now)` leaves out reservations expired at `now`.
- [ ] `status` at a time after a reservation expires shows it neither reserved nor holding stock.

**Out of scope:** the CLI output format, purging expired reservations, persistence.
