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

Check, in this order:

1. **Spec**: every requirement is implemented in full, and the change adds no behavior callers can see beyond what the unit asked for.
2. **Correctness**: empty, zero and boundary values; every error handled or passed on, not swallowed; state changes all-or-nothing; a second run, or a run after a crash, is safe.
3. **Tests**: every behavior and every branch the change adds has a test; a bug fix has a test that fails without it; tests go through the public interface and assert literal expected values.
4. **Security**: untrusted input is validated where it enters; none reaches a shell, a query, a path or a template unescaped; no secret sits in code, logs or fixtures.
5. **Architecture**: the change follows the repository's existing way to do each job, adds no second way, and removes the old path it replaces.

Trace the call path before you accept a hypothetical. Leave out style and personal preference.

Done when each axis has been checked against the diff.

## 2. Write the findings

One finding per defect:

- `file:line`
- what is wrong
- why it matters
- the fix

A finding is **Critical** when the change breaks, loses data, or opens a hole, and **Important** when it misses a requirement or a test the change needs. Write nothing below those two.

Done when every finding names a place in the diff and a concrete consequence.

## 3. End with the verdict

End with a last line that is exactly `VERDICT: PASS` when there are no findings, and exactly `VERDICT: FAIL` when there is at least one.
