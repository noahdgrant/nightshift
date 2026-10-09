---
unit: 3-cancel-older
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Add `Warehouse.cancel_older(sku, before)`, which drops every reservation of `sku` created strictly before `before` and returns how many it dropped.

**Acceptance criteria:**
- [ ] `cancel_older(sku, before)` removes every reservation of `sku` whose `created_at` is earlier than `before`, and returns the count.
- [ ] A reservation created exactly at `before`, or later, is kept.
- [ ] Reservations of other SKUs are untouched.
- [ ] An unknown SKU raises `UnknownItem`.

**Out of scope:** expiry rules, persistence.
