---
name: ns-principle-keep-scope-tight
description: "Apply on every diff in a unit. Fix what the diff causes, the rest of the class it fixes and small old bugs in the files it touches; file only unrelated or large findings."
disable-model-invocation: true
metadata:
  upstream: addyosmani/agent-skills@1401c8b8030e:skills/using-agent-skills
---

# Keep Scope Tight

Finish what the diff starts, and leave the rest of the codebase as it is. A finding is one of three kinds, and its kind decides whether the unit fixes it or files it.

**Why:** A drive-by fix grows the diff review must read, ties together changes that should ship apart, and lands work nobody agreed to. Filing what the diff itself broke is the opposite failure: the unit merges half done, and the follow-up issue costs a whole unit to close. The job is surgical precision: the diff's whole footprint, and nothing past it.

**Pattern:**
- **Trace every changed line** to an acceptance criterion, a test, refactor or removal one needs, or a finding below that the unit fixes.
- **Fallout: fix it.** Fallout is caused by this diff: a consumer of a new event, config key or format; docs and citations the change makes stale; baseline entries; a helper copy it adds; a file or function it pushes over the size bar; a form of a new ban it leaves out. Fix every one before the gate. Record each under its own `fallout:` line in `build.md`, with the commit that fixed it.
- **The rest of the class: fix it.** An instance of the brief's own defect class that the brief missed is the unit's to fix. File it only when fixing it would add more than about 150 changed lines, tagged `too-big: <lines>`.
- **Old bugs in touched files: fix the small ones.** A pre-existing bug in a file the diff already changes, fixable in about 30 changed lines, gets its own commit and test in the unit. A larger one is a `file:` line tagged `too-big: <lines>`.
- **File the rest.** A defect or missing work that is pre-existing and outside the diff's files, or larger than the bounds above, is a `file:` line under Open risks in `build.md`, with its location, evidence and one tag: `unrelated: <reason>`, or `too-big: <lines>` past a bound. `ns-ship` files it. On a file the diff touches, `unrelated:` holds only when the defect lies outside that file's code, and `ns artifact check` rejects a line there with neither tag.
- **Leave what no finding names as it is**: comments you don't understand, code beside the task, adjacent systems, code that only looks unused, features that seem useful.
- **Different, not more.** A change the brief asked for that you made differently goes under deviations, with the reason.

**Python:** the brief adds a `reservation_expired` event. `notify.py` dispatches on the event names and drops unknown ones, and `docs/events.md` lists them. Both are fallout: handle the event in `notify.py`, add it to the docs, and record `fallout: notify.py drops reservation_expired: handled in 4c1e2d0`. While there you see `stock_levels()` in `src/inventory/warehouse.py`, a file the diff doesn't touch, counting expired reservations. Leave it, and add `file: stock_levels counts expired reservations as reserved: src/inventory/warehouse.py, status shows 2 reserved after expiry, unrelated: reporting, not events`.

**Firmware:** the brief raises the UART baud rate. The driver beside it has a magic-number delay loop that waits too short at the new rate: fallout, fixed with the baud change. A CRC helper in a file the diff doesn't touch uses the wrong polynomial: record it for filing.

**Departs from upstream:** upstream's rule is "touch only what the brief requires" and files everything else. This skill reverses that for what the unit's own diff causes (fallout), the rest of the brief's defect class, and small old bugs in touched files, because filing them ships a half-done unit and costs a follow-up unit to close (ADR-747, `docs/decisions/747-a-unit-fixes-its-fallout.md`). When `ns-maintain` ports an upstream change to the scope rule, keep this split.
