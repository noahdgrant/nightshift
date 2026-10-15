---
unit: 5-reservation-note
phase: triage
status: pass
base: main
---
## Agent Brief

**Category:** enhancement

**Summary:** Let a reservation carry a short free-text note, such as the order it is for.

**Current behavior:** `Warehouse.reserve(sku, quantity, now, ttl)` records only the SKU, quantity and times. There is nowhere to say why stock is held.

**Desired behavior:** `Reservation` has an optional `note` field, `None` by default. `Warehouse.reserve` takes an optional `note=` keyword and stores it on the reservation it returns.

**Acceptance criteria:**
- [ ] `reserve(sku, quantity, now, note="order 1042")` returns a reservation whose `note` is `"order 1042"`, and `reservation(id).note` returns the same.
- [ ] `reserve` without `note` returns a reservation whose `note` is `None`.
- [ ] Existing calls to `reserve` work unchanged.

**Out of scope:** a CLI flag to set the note, showing the note in `status`.
