---
cycle: 1
kind: full
started: 2026-10-11T02:00:00Z
ended: 2026-10-11T02:10:00Z
---

# Fix cycle 1: 2-cancel-all

## Important
### I1. Test never checks that other SKUs' reservations survive cancel_all
- Location: `tests/test_warehouse.py:108`
- Axis: tests, spec
- Scope: changed
- Cycle: 0
- Attempts: 1
- Raised by: tests, spec
- Finding: acceptance criterion 2 says reservations of other SKUs are untouched, but the only cancel_all test reserves a single SKU.
- Evidence: replacing the filter with `self._reservations.clear()` passes the suite.
- Status: open. The build agent's fix in cycle 1 did not land: the test still reserves one SKU.
