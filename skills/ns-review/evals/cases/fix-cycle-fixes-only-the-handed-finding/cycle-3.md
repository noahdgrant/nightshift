---
cycle: 3
kind: full
---

# Fix cycle 3: 2-cancel-all

## Important
### I2. No test for cancel_all on a SKU with no reservations
- Location: `src/inventory/warehouse.py:119`
- Axis: tests
- Scope: changed
- Cycle: 1
- Attempts: 2
- Raised by: tests
- Finding: `cancel_all` on a known SKU with nothing reserved should return 0; no test covers the empty case.
- Evidence: changing `return len(doomed)` to `return len(doomed) or 1` passes the suite.
- Status: fixed (cycle 3, 5d6e7f8)

### I3. No test that cancel_all raises UnknownItem for an unknown SKU
- Location: `src/inventory/warehouse.py:118`
- Axis: tests, spec
- Scope: changed
- Cycle: 3
- Attempts: 0
- Raised by: tests, spec
- Finding: acceptance criterion 3 says an unknown SKU raises `UnknownItem`. `cancel_all` does so through `self.item(sku)`, but no test covers it, so deleting that line passes the suite.
- Evidence: mutation: delete `self.item(sku)` at line 118, run `timeout 60 python3 -m pytest -q tests/test_warehouse.py`: 26 passed (survived).
- Status: open
