---
schema: ns-artifact/2
unit: 2-cancel-all
phase: review
status: fail
cause: findings_open
next: review
reason: "Open in changed code: I3."
findings: [I3]
sha: HEADSHA
updated: 2026-10-11T02:30:00Z
base: main@BASESHA
head: HEADSHA
cycles: 3
panel: full
launch: subagents
---

# Review: 2-cancel-all

## Summary
Open: 0 Critical, 1 Important, 0 Suggestion. Fixed: 2. Dismissed: 0. Escapes filed: 0.
Change size: 30 lines in 2 files.
Panel: full, because the first pass's diff was over 50 changed lines with its tests.

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
