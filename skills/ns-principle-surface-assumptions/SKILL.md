---
name: ns-principle-surface-assumptions
description: "Apply before a non-trivial change whose brief leaves part of the requirements, architecture or scope unstated. Write down the assumptions the change rests on before acting; under gates: auto, record them in the artifact instead of asking."
disable-model-invocation: true
metadata:
  upstream: addyosmani/agent-skills@1401c8b8030e:skills/using-agent-skills
---

# Surface Assumptions

Before a non-trivial change, write down the assumptions it rests on. An unwritten assumption runs unchecked to the end of the unit. A written one costs a line, and anyone reading it can correct it.

**Why:** The most common failure is filling an ambiguous requirement silently and building on it. Rework found in review costs far more than a sentence of uncertainty up front.

**Pattern:**
- **List them before the first edit.** Requirements (what the brief means), architecture (where the code goes, what it may rely on), scope (what is in and out). One line each, concrete enough to be wrong.
- **Settle what a check can settle.** An assumption that a file read, a command or a test can confirm is a fact to look up. Look it up and drop it from the list.
- **Under `gates: stop`,** show the list and let the human correct it before you build.
- **Under `gates: auto`,** build on the list and record it in the phase's artifact, where review reads it. In `build.md` each one is a `note: assumes ...` line under Open risks. See [never block on the human](../ns-principle-never-block-on-the-human/SKILL.md).
- **An assumption that contradicts a source, or decides a design,** is confusion: [manage it](../ns-principle-manage-confusion/SKILL.md).

**Python:** the brief says "list the reservations for an item". Assumptions: "expired reservations are listed too, because `Warehouse.reservations()` keeps them until fulfilled or cancelled; the order is by reservation ID." Each line picks a test.

**Firmware:** the brief says "sample the ADC at 1 kHz". Assumptions: "1 kHz per channel, not across all four; the sampling timer's ISR may preempt the UART driver." A wrong first line is a 4x rate error, and a wrong second is a dropped byte.
