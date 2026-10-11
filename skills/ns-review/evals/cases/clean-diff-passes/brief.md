---
unit: 4-in-stock
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Add `Warehouse.in_stock(sku, now)`, true when at least one unit of `sku` can still be reserved at `now`.

**Acceptance criteria:**
- [ ] `in_stock` returns true when `available(sku, now)` is above zero, false otherwise.
- [ ] An unknown SKU raises `UnknownItem`.
- [ ] Tests cover both answers, at the boundary, and the unknown SKU.

**Out of scope:** the reservation rules themselves.
