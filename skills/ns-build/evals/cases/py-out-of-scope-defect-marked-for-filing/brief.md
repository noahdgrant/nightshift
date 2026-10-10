---
unit: 4-low-stock
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Add `Warehouse.low_stock(now, below)`, which returns the `StockLevel` of every item whose available quantity at `now` is less than `below`, sorted by SKU.

**Acceptance criteria:**
- [ ] `low_stock(now, below)` returns a `StockLevel` for each item whose available quantity at `now` is less than `below`, in SKU order, with the same fields `stock_levels(now)` reports.
- [ ] An item whose available quantity equals `below`, or is more, is left out.
- [ ] A reservation that has expired at `now` holds no stock: an item whose only reservation has expired is low only if its on-hand quantity is below `below`.

**Out of scope:** changing `stock_levels`, `available` or the CLI.
