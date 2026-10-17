---
name: ns-verify
description: Verify a built unit meets every requirement by running it the way a user would, changing nothing, and ending with a verdict line. Use after a unit is built, or when asked to prove that a change works.
metadata:
  upstream:
    - cursor/plugins@ccb5507cec15:pstack/skills/principle-prove-it-works
---

# Verify

"It compiles" is not evidence, and neither is "the tests pass". Run the real thing and record what you saw. You are given the unit's title and description and the build summary.

## 1. List the requirements

Number the requirements `R1..Rn` in the unit's order. For a bug, the original repro is a requirement of its own.

Done when every requirement has an ID and names the observable end state that would prove it.

## 2. Drive each requirement

Match the check to the change:

- A command-line change runs the real command.
- A service change sends the real request and reads the result back a second way.
- A parser or migration replays a saved real input.
- A bug fix reruns the original repro.

For each requirement, record the command, an output excerpt that shows the end state, and a verdict:

- **PASS**: you observed the end state.
- **FAIL**: you observed a different end state.
- **INCONCLUSIVE**: the check could not run, or the evidence does not show the end state. Say what would make it checkable.

When a check fails, suspect how you looked before you call the work broken.

Done when every requirement has a verdict, and every PASS and FAIL has a command and an excerpt.

## 3. Leave the workspace as you found it

Create what a check needs outside the workspace, in a temporary directory, and remove it after. Change no file in the workspace.

When you are told your last check went wrong, undo what you changed first, then check again.

## 4. End with the verdict

Write the evidence: one section per requirement. End with a last line that is exactly `VERDICT: PASS` when every requirement is PASS, and exactly `VERDICT: FAIL` otherwise.
