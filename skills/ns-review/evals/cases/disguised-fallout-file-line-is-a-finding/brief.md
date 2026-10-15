---
unit: 01-reservation-note
phase: triage
status: pass
base: main
---
## Agent Brief

**Issue:** `.scratch/reservations/issues/01-reservation-note.md`

**Summary:** A reservation can carry a short free-text note, such as the order it is for.

**Acceptance criteria:**
- [ ] `Warehouse.reserve` takes an optional `note`, and the `Reservation` it returns carries it.
- [ ] A reservation made without a note has `note` set to `None`.
- [ ] `reserve` raises `ValueError` for a blank note, and holds nothing.

**Out of scope:** the CLI.
