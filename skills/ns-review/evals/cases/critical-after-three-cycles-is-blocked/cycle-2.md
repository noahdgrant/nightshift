---
cycle: 2
kind: full
started: 2026-10-11T02:10:00Z
ended: 2026-10-11T02:20:00Z
---

# Fix cycle 2: 2-cancel-all

## Critical
### C1. cancel_all drops reservations of every SKU
- Location: `src/inventory/warehouse.py:119`
- Axis: correctness, spec
- Scope: changed
- Cycle: 0
- Attempts: 2
- Raised by: correctness, spec
- Finding: `cancel_all` counts and clears `self._reservations` without filtering on `sku`, so cancelling one SKU destroys every other SKU's holds.
- Evidence: reserve NUT-M6 and BOLT-M6, call `cancel_all("BOLT-M6")`; `reservations("NUT-M6")` is empty.
- Status: open. The build agent's fix in cycle 2 did not land: the code at this location is unchanged.
