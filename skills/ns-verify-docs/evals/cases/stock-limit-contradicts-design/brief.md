---
unit: 8-tighten-stock-3
phase: triage
status: pass
base: main
---
## Agent Brief

**Category:** docs

**Summary:** The night batch ran late, so Priya tightened STOCK-3: `reserve` and `fulfil` may now be at most 5% slower, not 10%. Record it in the stock requirements, which are still in review.

**Acceptance criteria:**
- [ ] STOCK-3 in `docs/requirements/stock/v1.md` caps the slowdown at 5%, and its source names Priya's 2026-10-05 decision.
- [ ] `docs/requirements/stock/changelog.md` has a line for the change, and the doc's Last updated is 2026-10-05.

**Out of scope:** the code, and editing design docs or tickets in this unit.
