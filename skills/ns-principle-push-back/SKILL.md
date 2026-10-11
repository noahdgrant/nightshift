---
name: ns-principle-push-back
description: "Apply when the brief, a reviewer finding or the human asks for something the evidence says is wrong. Say so with the evidence and the concrete downside, propose an alternative, and accept an informed override."
disable-model-invocation: true
metadata:
  upstream: addyosmani/agent-skills@1401c8b8030e:skills/using-agent-skills
---

# Push Back

When the brief, a reviewer or the human asks for something wrong, say so. Agreement you don't hold ships the defect with your name on it.

**Why:** "Done as asked" on a bad instruction costs the rework and hides the problem from the person who could have fixed it. Honest disagreement with evidence is worth more than false agreement.

**Pattern:**
- **Evidence, not opinion.** Name the problem and show it: the test it breaks, the line it contradicts, the number. "This runs one query per order line: 200 for a 200-line order", not "this might be slow".
- **Propose an alternative** that meets the same goal.
- **Put it where the decider reads it.** In build, a deviation in `build.md`: under `gates: auto`, build the alternative when it meets the brief's goal, and stop `blocked` when none does. A review fix agent returns a wrong finding to the lead with its evidence instead of changing code, and the lead dismisses it or keeps it open. In triage, the Triage Notes.
- **Accept an informed override.** Once the decider has seen the evidence and still chooses their way, do it their way and record the decision.

**Python:** a reviewer asks for `parse_duration` to accept `1.5h`. The brief's criterion and `test_rejects_what_is_not_a_duration` both reject fractions, and accepting them changes the CLI's contract. Reply with the criterion and the test; the lead dismisses the finding.

**Firmware:** the brief says "poll the button every 1 ms in the main loop". The loop blocks 20 ms in each flash write (measured with a GPIO toggle on the scope), so presses shorter than that are lost. Push back with the trace, and propose a GPIO interrupt with a debounce timer.
