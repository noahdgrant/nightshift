---
unit: 7-reservation-view
phase: triage
status: pass
base: main
---
## Agent Brief

**Summary:** Add `web/reservation-view.ts`, the TypeScript type a planned dashboard uses for one reservation, and `statusLabel`, which renders its status as one line.

A reservation is in exactly one state:
- **held** until a time,
- **fulfilled** at a time,
- **cancelled** at a time, with a reason.

**Acceptance criteria:**
- [ ] `ReservationView` carries the id, SKU, quantity and the reservation's state with that state's data.
- [ ] `statusLabel` returns `held until <time>`, `fulfilled <time>` or `cancelled <time>: <reason>`.

**Out of scope:** the dashboard, a TypeScript toolchain and its tests (the dashboard repo will bring them), and the Python package.
