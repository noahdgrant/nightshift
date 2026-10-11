---
name: ns-principle-manage-confusion
description: "Apply when the brief, the code, the docs or a test contradict each other, or a fact the work needs is missing. Stop and resolve it by reading, reproducing, asking, or recording and routing, instead of guessing."
disable-model-invocation: true
metadata:
  upstream: addyosmani/agent-skills@1401c8b8030e:skills/using-agent-skills
---

# Manage Confusion

Confusion is a signal. When two sources disagree, or the fact you need isn't written anywhere, stop before the next edit and resolve it. A guess buries the conflict inside the diff, where review sees only the result.

**Why:** Silently picking one reading is a coin flip that looks like progress.

**Pattern:**
1. **Name it in one sentence.** "The brief says X; the code does Y." "Nothing says what happens to Z on timeout."
2. **Resolve it, cheapest first:**
   - **Read**: the domain docs, the glossary, ADRs, `git log -S`, the issue thread. Often one source is just stale.
   - **Reproduce**: run the code, the test or the command. Observed behaviour outranks a description of it.
   - **Ask**: under `gates: stop`, put the sentence and the options to the human, and wait.
   - **Record and route**: under `gates: auto`, or with nobody to ask, write the sentence and your resolution into the artifact. Take the reading that keeps existing tested behaviour and record it as a deviation. When the conflict changes what the unit is for, or opens a [design decision](../ns-triage/SKILL.md#design-decisions), stop the phase `blocked` and route it to `ns-define`.
3. **Continue** once it is resolved or recorded.

**Python:** the brief says `reserve` raises `OutOfStock`, but `errors.py` has only `InsufficientStock` and `test_cli.py` matches its message. `git log -S OutOfStock` shows the rename. The brief is stale: use `InsufficientStock` and record the deviation.

**Firmware:** the datasheet gives the sensor's I2C address as 0x48, the devicetree says 0x49. Reproduce: scan the bus on the board, or read the ADDR strap in the schematic. Fix whichever source is wrong, and record which.
