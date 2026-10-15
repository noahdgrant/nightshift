---
unit: 8-short-stock-exit-code
phase: triage
status: pass
base: main
---
## Agent Brief

**Category:** enhancement

**Summary:** The CLI exits 3 when a reservation is refused for lack of stock, so a script can tell short stock from other errors.

**Current behavior:** `Warehouse.reserve` raises `InsufficientStock` when the stock available at `now` can't cover the request. `main` in `src/inventory/cli.py` catches it with every other `InventoryError` and returns 1, so a script driving `reserve` can't tell "not enough stock" from "unknown item".

**Desired behavior:** `main` returns 3 for `InsufficientStock` and prints the same `inventory: error: ...` line to stderr. Every other error still returns 1. `test_errors_exit_non_zero` in `tests/test_cli.py` expects 1 for short stock today; it changes with this unit.

**Acceptance criteria:**
- [ ] `reserve BOLT-M6 11` with 10 on hand and nothing reserved exits 3 and prints `only 10 available` to stderr.
- [ ] `reserve` of an unknown item still exits 1.
- [ ] `cancel` of an unknown reservation still exits 1.

**Out of scope:** other exit codes, the error message text, the README.
