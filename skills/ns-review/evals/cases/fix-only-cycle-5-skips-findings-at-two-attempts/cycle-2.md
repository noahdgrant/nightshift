---
cycle: 2
kind: full
started: 2026-10-11T02:10:00Z
ended: 2026-10-11T02:20:00Z
---

# Fix cycle 2: 2-cancel-all

## Important
### I1. Test never checks that other SKUs' reservations survive cancel_all
- Location: `tests/test_warehouse.py:108`
- Axis: tests, spec
- Scope: changed
- Cycle: 0
- Attempts: 2
- Raised by: tests, spec
- Finding: acceptance criterion 2 says reservations of other SKUs are untouched, but the only cancel_all test reserves a single SKU.
- Evidence: replacing the filter with `self._reservations.clear()` passes the suite.
- Status: fixed (cycle 2, 1a2b3c4)

### I3. No test that cancel_all raises UnknownItem for an unknown SKU
- Location: `src/inventory/warehouse.py:118`
- Axis: tests, spec
- Scope: changed
- Cycle: 2
- Attempts: 0
- Raised by: tests, spec
- Finding: acceptance criterion 3 says an unknown SKU raises `UnknownItem`. `cancel_all` does so through `self.item(sku)`, but no test covers it, so deleting that line passes the suite.
- Evidence: mutation: delete `self.item(sku)` at line 118, run `timeout 60 python3 -m pytest -q tests/test_warehouse.py`: 26 passed (survived).
- Status: open
