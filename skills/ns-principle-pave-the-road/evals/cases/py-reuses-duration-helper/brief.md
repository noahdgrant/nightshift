---
unit: 5-extend-reservation
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Let a held reservation be extended: `Warehouse.extend(reservation_id, by, now)` in the domain, and `inventory extend <reservation-id> <duration>` on the command line.

**Acceptance criteria:**
- [ ] `Warehouse.extend(reservation_id, by, now)` takes a `timedelta`, moves the reservation's `expires_at` later by `by`, and returns the updated reservation. Its quantity, SKU and `created_at` are unchanged.
- [ ] Extending a reservation that has expired at `now` raises `ReservationExpired`; an unknown id raises `UnknownReservation`.
- [ ] `inventory extend R0001 2h` extends R0001 by two hours, saves the state file, and prints the reservation the way `reserve` does: `R0001: 2 x BOLT-M6 until 2026-03-02T11:15:00`.
- [ ] The duration is a number followed by `m`, `h` or `d` (`45m`, `2h`, `1d`). A malformed or zero duration exits non-zero with an error and leaves the state file unchanged.

**Out of scope:** extending by a negative duration, changing a reservation's quantity.
