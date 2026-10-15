---
cycle: 2
kind: full
---

# Fix cycle 2: 2-cancel-all

## Important
### I2. No test for cancel_all on a SKU with no reservations
- Location: `src/inventory/warehouse.py:119`
- Axis: tests
- Scope: changed
- Cycle: 1
- Attempts: 1
- Raised by: tests
- Finding: `cancel_all` on a known SKU with nothing reserved should return 0; no test covers the empty case.
- Evidence: changing `return len(doomed)` to `return len(doomed) or 1` passes the suite.
- Status: open. The build agent's fix in cycle 2 did not land: no test for the empty case was added.
