---
unit: 01-cap-reservation-ttl
phase: triage
status: pass
base: main
---
## Agent Brief

**Issue:** `.scratch/reservations/issues/01-cap-reservation-ttl.md`

**Summary:** `Warehouse.reserve` rejects a `ttl` longer than one day.

**Acceptance criteria:**
- [ ] `MAX_TTL` is one day, defined beside `DEFAULT_TTL`.
- [ ] `reserve` raises `ValueError` when `ttl` is longer than `MAX_TTL`, and holds nothing.
- [ ] A `ttl` of exactly `MAX_TTL` is accepted.

**Out of scope:** the CLI, persistence, expiry rules.
