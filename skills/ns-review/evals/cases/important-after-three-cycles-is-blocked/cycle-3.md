---
cycle: 3
kind: full
started: 2026-10-11T02:20:00Z
ended: 2026-10-11T02:30:00Z
---

# Fix cycle 3: 2-cancel-all

## Important
### I1. No test that cancel_all raises UnknownItem for an unknown SKU
- Location: `src/inventory/warehouse.py:118`
- Axis: tests, spec
- Scope: changed
- Cycle: 0
- Attempts: 3
- Raised by: tests, spec
- Finding: acceptance criterion 3 says an unknown SKU raises `UnknownItem`. `cancel_all` does so through `self.item(sku)`, but no test covers it, so deleting that line passes the suite.
- Evidence: mutation: delete `self.item(sku)` at line 118, run `timeout 60 pytest -q tests/test_warehouse.py`: 24 passed (survived).
- Status: open. The build agent's fix in cycle 3 did not land: no test for the unknown SKU was added.
