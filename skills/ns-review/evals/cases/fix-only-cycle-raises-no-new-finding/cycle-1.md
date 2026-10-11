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
- Status: fixed (cycle 1, 1a2b3c4)

### I2. No test for cancel_all on a SKU with no reservations
- Location: `src/inventory/warehouse.py:119`
- Axis: tests
- Scope: changed
- Cycle: 1
- Attempts: 0
- Raised by: tests
- Finding: `cancel_all` on a known SKU with nothing reserved should return 0; no test covers the empty case.
- Evidence: changing `return len(doomed)` to `return len(doomed) or 1` passes the suite.
- Status: open
