---
name: ns-principle-keep-scope-tight
description: "Apply on every diff in a unit. Touch only what the brief requires, and record everything else worth doing for filing as an issue instead of fixing it in passing."
disable-model-invocation: true
metadata:
  upstream: addyosmani/agent-skills@1401c8b8030e:skills/using-agent-skills
---

# Keep Scope Tight

Touch only what the brief requires. Everything else worth doing becomes an issue.

**Why:** A drive-by fix grows the diff review must read, ties together changes that should ship apart, and lands work nobody agreed to. The job is surgical precision, not renovation.

**Pattern:**
- **Trace every changed line** to an acceptance criterion, or to a test, refactor or removal one needs.
- **Leave what the brief doesn't name as it is**: comments you don't understand, code beside the task, adjacent systems, code that only looks unused, features that seem useful.
- **Record the rest for filing.** A defect or missing work outside the brief goes in the artifact with its location and evidence. In `build.md` it is a `file:` line under Open risks, and `ns-ship` files it.
- **Different, not more.** A change the brief asked for that you made differently goes under deviations, with the reason.

**Python:** the brief adds `inventory extend`. While writing it you see `stock_levels()` counting expired reservations. Leave `stock_levels()` as it is, and add `file: stock_levels counts expired reservations as reserved: src/inventory/warehouse.py, status shows 2 reserved after expiry`.

**Firmware:** the brief raises the UART baud rate. The driver beside it has a magic-number delay loop. Change only the baud setting and its test, and record the delay loop for filing.
