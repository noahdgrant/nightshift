---
cycle: 4
kind: fix-only
started: 2026-10-11T02:30:00Z
ended: 2026-10-11T02:40:00Z
---

# Fix cycle 4: 2-cancel-all

## Important
### I3. No test that cancel_all raises UnknownItem for an unknown SKU
- Location: `src/inventory/warehouse.py:118`
- Axis: tests, spec
- Scope: changed
- Cycle: 2
- Attempts: 2
- Raised by: tests, spec
- Finding: acceptance criterion 3 says an unknown SKU raises `UnknownItem`. `cancel_all` does so through `self.item(sku)`, but no test covers it, so deleting that line passes the suite.
- Evidence: mutation: delete `self.item(sku)` at line 118, run `timeout 60 python3 -m pytest -q tests/test_warehouse.py`: 26 passed (survived).
- Status: open. The build agent's fix in cycle 4 did not land: the tests reviewer found no test calling cancel_all with an unknown SKU.

### I4. No test that stock levels recover after cancel_all
- Location: `tests/test_warehouse.py:112`
- Axis: tests
- Scope: changed
- Cycle: 3
- Attempts: 1
- Raised by: tests
- Finding: the cancel_all tests assert the returned count and `reservations()`, but never that `stock_levels` shows the cancelled quantity as available again.
- Evidence: replacing the `del self._reservations[rid]` loop with a no-op that returns `len(doomed)` fails only the `reservations()` assertions; nothing asserts `available` after the cancel.
- Status: open. The build agent's fix in cycle 4 did not land: no test reads `stock_levels` after cancel_all.
