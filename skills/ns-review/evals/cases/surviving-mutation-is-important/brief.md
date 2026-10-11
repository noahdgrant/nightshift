---
unit: 5-low-stock
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Add `Warehouse.low_stock(now, threshold=3)`, the SKUs that are running out.

**Acceptance criteria:**
- [ ] `low_stock` returns, in SKU order, every SKU with at most `threshold` units available at `now`.
- [ ] An item with exactly `threshold` units available counts as low.

**Out of scope:** the CLI, alerts.
