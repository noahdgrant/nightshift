---
name: ns-build
description: Build a unit. Implement it test-first in thin vertical slices, run the repository's checks, and leave exactly one commit. Use when a unit is ready to build, or when asked to implement an issue.
metadata:
  upstream:
    - mattpocock/skills@b0618bc436ad:skills/engineering/implement
    - addyosmani/agent-skills@1401c8b8030e:skills/incremental-implementation
---

# Build

Turn one unit into one commit. You are given the unit's title and description, the commit the branch started from, and how to run the repository's checks. Your final message is the build summary.

## 1. Read the unit

Read the files you will change, their tests, and one example of the pattern you will follow. Map each requirement to a place a test can observe it.

When the unit leaves something open, take the simplest reading and name it under **Assumptions** in your summary. When the unit, the code, the docs or a test disagree, or a fact is missing, do not guess silently: take the reading that changes least and say what you assumed.

Done when every requirement has a seam for its test.

## 2. Build in vertical slices

Run the test-driven loop in the instructions that follow these, one requirement per slice, the riskiest first. Each slice ends with its test green and every existing test green.

Keep the diff to what the unit asks: touch only what it names, and put anything else you find under **Noticed** in your summary instead of fixing it. Fix what your change breaks. Subtract before you add: prefer deleting or reusing to a new abstraction, layer or option, and make the smallest change that meets the unit. Test behavior, not implementation: a test goes through the public interface and asserts literal expected values.

Done when every requirement has a passing test.

## 3. Run the checks

Run the repository's checks the way you were told, and fix what they report until they pass. The same checks run again after you finish, so a pass here is a pass there.

Done when the checks pass on the tree you will commit.

## 4. Leave one commit

Commit all the work as one commit. The subject says what changed in the repository's commit style, and the body says why. When you made more than one commit, squash them into one.

Done when the branch holds exactly one commit past its starting commit and nothing is uncommitted.

## 5. Write the summary

End with a short summary:

- **Changed**: what the commit does.
- **Tests**: each requirement and the test that covers it.
- **Assumptions** and **Noticed**, when you have any.

## When told something went wrong

A later message names what was left wrong: uncommitted changes, extra commits, failed checks, or findings from a check of your work. Fix exactly that, then run steps 3 and 4 again.
