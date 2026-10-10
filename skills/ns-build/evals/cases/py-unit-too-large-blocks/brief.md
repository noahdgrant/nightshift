---
unit: 5-renew-and-release
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Let a holder renew a reservation before it lapses, let an operator clear out lapsed reservations, and expose both on the CLI.

**Acceptance criteria:**
- [ ] `Warehouse.renew(reservation_id, now, ttl)` moves the reservation's `expires_at` to `now + ttl` and returns the renewed `Reservation`. A reservation that has expired at `now` raises `ReservationExpired`; an unknown ID raises `UnknownReservation`; a `ttl` of zero or less raises `ValueError`.
- [ ] `Warehouse.release_expired(now)` drops every reservation that has expired at `now` and returns them in ID order. Live reservations are kept.
- [ ] `inventory renew <reservation_id> [--ttl MINUTES]` renews a reservation (default 30 minutes), prints `<id>: renewed until <expires_at ISO 8601>`, and saves the state file.
- [ ] `inventory release-expired` drops the expired reservations, prints `released <id>` for each one, and saves the state file.

**Out of scope:** changing `reserve`, `stock_levels` or the state file format.
