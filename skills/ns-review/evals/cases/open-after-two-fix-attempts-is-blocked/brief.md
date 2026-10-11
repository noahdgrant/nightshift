---
unit: 2-cancel-all
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Add `Warehouse.cancel_all(sku)`, which drops every reservation of `sku` and returns how many it dropped.

**Acceptance criteria:**
- [ ] `cancel_all(sku)` removes every reservation of `sku` and returns the count.
- [ ] Reservations of other SKUs are untouched.
- [ ] An unknown SKU raises `UnknownItem`.

**Out of scope:** expiry rules, persistence.
