---
name: ns-review
description: Review a unit's change for what must be fixed before it merges, and end with a verdict line. Use after a unit is verified, or when asked to review a branch or diff.
metadata:
  upstream:
    - addyosmani/agent-skills@1401c8b8030e:skills/code-review-and-quality
    - mattpocock/skills@b0618bc436ad:skills/engineering/code-review
---

# Review

Find what must be fixed before this change merges. You are given the unit's title and description, the build summary and the full diff. You read and change nothing.

## 1. Read the change against the unit

There are six review areas. A section `# Your focus` in the prompt names the ones you cover; review only those and leave the others to other sessions. With no such section, review all six, in this order.

### Correctness

Empty, zero and boundary values; every error handled or passed on, not swallowed; state changes all-or-nothing; a second run, or a run after a crash, is safe.

### Security

Untrusted input is validated where it enters; none reaches a shell, a query, a path or a template unescaped; no secret sits in code, logs or fixtures.

### Spec

Every requirement is implemented in full, and the change adds no behavior callers can see beyond what the unit asked for.

### Tests

Every behavior and every branch the change adds has a test; a bug fix has a test that fails without it; tests go through the public interface and assert literal expected values.

### Architecture

The change follows the repository's existing way to do each job, adds no second way, and removes the old path it replaces.

### Docs and comments

The comments and docs the change adds or touches are true and say why, not what; the docs the change makes stale are updated.

Trace the call path before you accept a hypothetical. Leave out style and personal preference.

Done when each area you cover has been checked against the diff.

## 2. Check the earlier findings

When the prompt has a section `# Findings from the previous review`, give each finding in it one status and a reason:

- **Fixed**: say how you checked, by the diff or the tests, not by the build summary.
- **Still open**: the defect is still in the change.
- **Withdrawn**: the finding was wrong; say why.

Do not accept as harmless what that review called Important unless you withdraw it and say why. The findings in your prompt are the ones from your own areas; judge only those.

Done when every earlier finding has a status with a reason.

## 3. Write the new findings

List them apart from the earlier ones. One finding per defect:

- `file:line`
- what is wrong
- why it matters
- the fix

A finding is **Critical** when the change breaks, loses data, or opens a hole, and **Important** when it misses a requirement or a test the change needs. Write nothing below those two.

A finding is Important only when the change fails the unit's requirements, breaks behavior that works today, loses or leaks data, or is unsafe. A failure that needs a file edited by hand, or a state `ns` never writes, is not one.

You run nothing, so a claim about what happens when the code runs (a test fails, a command errors, output differs) is a guess unless you quote where you were given it, such as the build summary or the evidence. Otherwise word it as a risk read from the code ("this test would fail because ..."), never as a fact. A finding that is only a guess about a run is not Critical or Important.

Done when every finding names a place in the diff and a concrete consequence.

## 4. End with the verdict

End with a last line that is exactly `VERDICT: FAIL` when any earlier finding in your areas is still open or you found a new one there, and exactly `VERDICT: PASS` when none is.
