---
unit: 6-expired-holds
phase: triage
status: pass
base: main
---
## Agent Brief

**Category:** bug

**Summary:** Stock levels count reservations that have expired, so they show stock as held after the hold ends.

**Current behavior:** `Warehouse.stock_levels(now)` sums every reservation per SKU without checking expiry, so `StockLevel.reserved` and `.available` (and the CLI `status`) still include a reservation after it expires. `Warehouse.reserved()` and `available()` go through `_active()` and are correct.

**Desired behavior:** stock levels count only the reservations still held at `now`, through the same active-reservation logic `reserved()` uses.

**Instances:**
- Search: `grep -n "reserved_by_sku" src/` for every per-SKU sum of held reservations
- `Warehouse.stock_levels()`: the reported instance, covered
- `Warehouse._active()` and `reservations()`: no defect (`_active` filters expiry; `reservations()` lists expired holds by design)

**Failure modes:** none; `stock_levels` is a pure read of in-memory state.

**Edge cases:**
- A reservation at exactly its `expires_at` no longer counts (`Reservation.is_expired` is `now >= expires_at`)
- An item with no reservations reports 0 reserved

**Acceptance criteria:**
- [ ] `stock_levels(now)` leaves out reservations expired at `now`.
- [ ] `status` at a time after a reservation expires shows it neither reserved nor holding stock.

**Out of scope:** the CLI output format, purging expired reservations, persistence.
