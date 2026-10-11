---
unit: 3-purge-expired
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Add `Warehouse.purge_expired(now)`, which drops every reservation that has expired at `now` and returns how many it dropped.

**Acceptance criteria:**
- [ ] `purge_expired(now)` removes every reservation expired at `now` and returns the count.
- [ ] Reservations still held at `now` are untouched.

**Out of scope:** the CLI, persistence.
