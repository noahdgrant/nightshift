---
unit: 1-in-stock
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Add `Warehouse.in_stock(sku, now)`, true when at least one unit of `sku` can still be reserved at `now`.

**Acceptance criteria:**
- [ ] `in_stock` returns true when `available(sku, now)` is above zero, false otherwise.
- [ ] A test covers both answers.

**Out of scope:** the reservation rules themselves.
