---
schema: ns-artifact/2
unit: 01-reservation-note
phase: build
status: pass
cause: none
next: verify
reason: "All three criteria pass: reserve takes an optional note and rejects a blank one."
sha: @SHA@
updated: 2026-10-14T02:41:00Z
---

## Commits
@SHA@ feat(warehouse): let a reservation carry a note

## Tests added
tests/test_warehouse.py::test_reserve_keeps_the_note: AC1 (Warehouse API)
tests/test_warehouse.py::test_reserve_without_a_note_has_none: AC2 (Warehouse API)
tests/test_warehouse.py::test_reserve_rejects_a_blank_note: AC3 (Warehouse API)

## Checks
python3 -m pytest -q: pass, `28 passed in 0.09s`

## Self-check
Bar: 9 met, 0 fixed, 3 not applicable. performance: no hot path touched. security: the note is stored, never run or used as a path. comments: none added.
Branches: 2 listed, all tested. src/inventory/warehouse.py `note is not None and not note.strip()`: test_reserve_rejects_a_blank_note. `note is None`: test_reserve_without_a_note_has_none.

| Mutation | Location | Command | Result |
|---|---|---|---|
| drop `note=note` | `src/inventory/warehouse.py:109` | `timeout 60 python3 -m pytest -q tests/test_warehouse.py` | killed |
| `not note.strip()` to `False` | `src/inventory/warehouse.py:98` | `timeout 60 python3 -m pytest -q tests/test_warehouse.py` | killed |

## Deviations from the brief
none

## Open risks
- file: Reservation.to_dict and Reservation.from_dict don't write or read `note`, so a saved warehouse loads its reservations without their notes: src/inventory/models.py:43, storage round trip of a noted reservation gives note None, unrelated: persistence is a separate feature the brief doesn't ask for
