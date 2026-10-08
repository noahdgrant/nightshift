---
name: ns-no-comments
description: Strip comments and lint/type suppressions from a diff with the Comment Sicko persona, fix accepted MUST KILLs at the root cause, and encode constraint comments as types, assertions, tests or lints. Use when cleaning comments out of a change, when a review flags comments or suppressions, or before shipping.
metadata:
  upstream:
    - cursor/plugins@ccb5507cec15:pstack/skills/no-comments
    - cursor/plugins@ccb5507cec15:pstack/agents/comment-sicko.md
---

# No comments

Run Comment Sicko on the scope, audit its report, and act on what survives the audit. Defer to its fresh perspective: it has not read the code's history, so it sees the comments the way the next reader will.

## Scope

Use the caller's files or diff. Otherwise use the current diff against the base branch, working tree included: `git diff "$(git merge-base <base> HEAD)"`, with the base from `.ns/<unit-id>/brief.md` or the repo default branch.

## Steps

1. **Spawn Comment Sicko.** Hand [comment-sicko.md](references/comment-sicko.md) and the scope to a **fresh-context agent**. Pass the persona file as written and add nothing to its rules. A subagent may edit comments directly. Through `ns ask --role review.comments` it is read-only, so it runs in report mode and you apply the deletions it lists. Inline (no subagent, no `ns`), read the persona and apply it yourself, and say in the report that it ran inline.

   Done when you hold its report and, for a subagent, its diff.

2. **Audit the report.** Reject:
   - edits to application code, or to anything outside the scope
   - deletions the keep-list protects
   - `MUST KILL` reasons that misstate the code
   - flags on intentional code kept for a proven reason

   A reshape flag on a surprise in our own code stays actionable; leave that comment deleted. A keep survives only with proof it is about something we cannot change. Firmware register, errata and datasheet-constraint comments qualify under the external platform/vendor exception. Keep them, and prefer that they name the document and section (`RM0090 28.6.3`, `errata ES0182 2.1.4`).

   Check for suppressions it missed in scope: `# noqa`, `# type: ignore`, `# pragma: no cover`, `// NOLINT`, `#pragma` diagnostic pragmas, `/* clang-format off */`. Suppressions over a correctness or safety rule stay actionable `MUST KILL`s.

   Before accepting a thin `IMPORTANT` or `do not remove` kill or keep, trace the named symbol: callers, definition, `git log -L` on the lines. An ambiguous kill stays killed. A keep that is refuted or still ambiguous is deleted. Restore a deletion only with the exact keep-list clause and proof in scope.

   If you reject the report, revert its edits and rerun Comment Sicko once with the failure named. If the second report also fails the audit, report it open and stop with `fail`.

   Done when every deletion and every flag is accepted or rejected with a reason.

3. **Fix trivial flags directly**: delete a dead path, drop an unused parameter, call the real API, name the rule a suppression was hiding and fix what it caught.

4. **Fix MUST KILLs at the root cause.** Implement the smallest root-cause fix in scope and remove every named workaround: the rename, extraction, type or reshape that makes the code say what the comment said. If a fix needs a new shape, sketch it for the whole accepted set once before editing. If the root cause sits outside the scope, land the smallest in-scope fix and report the rest open. [Fix root causes](../ns-principle-fix-root-causes/SKILL.md) and [redesign from first principles](../ns-principle-redesign-from-first-principles/SKILL.md) set the intent only. They do not widen the scope. Fix the cause, never add a symptom guard.

   Behaviour changes follow the `ns-tdd` skill: a failing test first.

5. **Encode constraint comments.** A constraint comment says `do not remove`, `do not change`, or `talk to X before changing`. Leave keeps about things we cannot change. For the rest, offer the cheapest in-scope enforcement:
   - a **type** that makes the wrong value unrepresentable (an `Enum`, a `NewType`, a frozen dataclass)
   - an **assertion** or runtime check at the boundary
   - a **test** that goes red when the constraint breaks
   - a **lint** rule or CI check

   Under `gates: stop`, wait for the human to approve each offer. Under `gates: auto`, encode without asking. If approved, encode and then delete the comment. Otherwise delete it, report the constraint open, and sketch the out-of-scope work. See [encode lessons in structure](../ns-principle-encode-lessons-in-structure/SKILL.md).

   ```python
   # before
   TIMEOUT_MS = 250  # do not go below 200, the modem drops frames

   # after: config.py
   MIN_MODEM_TIMEOUT_MS = 200  # modem datasheet 4.2: frames drop below this
   TIMEOUT_MS = 250

   # tests/test_config.py
   def test_timeout_respects_modem_minimum():
       assert TIMEOUT_MS >= MIN_MODEM_TIMEOUT_MS
   ```

   Firmware: a compile-time check (`_Static_assert`, a linker-script `ASSERT`) beats a runtime assertion for register layouts, buffer sizes and memory-map constraints.

6. **Verify.** Run the test and lint commands from `docs/agents/stack.md`. Record each command and its result.

## Report

Return to the caller:

- launch mode (subagent, `ns ask`, inline)
- deletion count and touched files
- restored comments, with the keep-list clause for each
- reruns, and why
- the sketch, if step 4 needed one
- fixes made, with `file:line`
- encoding offers, and which were encoded
- unenforced constraints and other open work
- the verify commands and their output
