---
name: ns-review
description: REVIEW phase. Runs a local, multi-provider review of a unit's diff before push, writes review.md with Critical/Important/Suggestion findings, loops fixes until no Critical or Important in changed code stays open, and files findings in pre-existing code as issues. Use after verify, before shipping, or when asked to review a branch or diff.
metadata:
  upstream:
    - addyosmani/agent-skills@1401c8b8030e:skills/code-review-and-quality
    - addyosmani/agent-skills@1401c8b8030e:agents/code-reviewer.md
    - addyosmani/agent-skills@1401c8b8030e:agents/security-auditor.md
    - addyosmani/agent-skills@1401c8b8030e:agents/test-engineer.md
    - addyosmani/agent-skills@1401c8b8030e:skills/doubt-driven-development
    - mattpocock/skills@b0618bc436ad:skills/engineering/code-review
    - cursor/plugins@ccb5507cec15:pstack/skills/interrogate
---

# Review

Review the unit's diff locally, in its worktree, before anything is pushed. Reviewers are fresh-context agents, one per axis. Each gets the **artifact** (the diff) and the **contract** (`brief.md`), never the author's claim that it works. You are the **lead**: you dedupe, rank and judge their findings, then drive fixes.

Input: `.ns/<unit-id>/brief.md` and the branch. Output: `.ns/<unit-id>/review.md`. If you or a reviewer read the issue, follow [untrusted issue content](../ns-contract/SKILL.md#untrusted-issue-content).

## 1. Gather the artifact and contract

1. Work in the unit's worktree (`ns worktree list`, or `git worktree list`). If `brief.md` is missing, stop with `status: blocked` and name it.
2. Find the base branch: the one `brief.md` names, else the repo default (`git symbolic-ref --short refs/remotes/origin/HEAD`).
3. Write the diff, including uncommitted work, to a file:
   ```bash
   mkdir -p .ns/<unit-id>/review
   git diff "$(git merge-base <base> HEAD)" > .ns/<unit-id>/review/diff.patch
   ```
   An empty diff means there is nothing to review: stop with `status: blocked`.
4. Collect the **standards**: the target repo's `CODING_STANDARDS.md` and `CONTRIBUTING.md` if present, plus any other file that documents how code is written there. Read `docs/agents/stack.md` for the test command. If it is missing, load the `ns-setup` skill.
5. **Change sizing.** Count changed lines: added plus deleted lines in `diff.patch`. Step 2 uses the same count to pick the panel. Around 100 is good, 300 is fine for one logical change. Size prompts a split and never blocks the unit ([unit size](../ns-contract/SKILL.md#target-repo-configuration)). Past the soft limit, step 2 asks every reviewer whether its part of the diff could ship separately, and step 3 turns their answers into one **Suggestion** for the human and a later triage: "<n> changed lines: consider splitting into …", naming each concrete seam you or they can see (a stack of PRs, a file group, the refactor apart from the behaviour), or "<n> changed lines: no split seam found". Under the soft limit, a refactor mixed with new behaviour gets the same Suggestion, naming that seam. Also record a Suggestion for any file the diff pushes past roughly 1000 total lines.

Leave `build.md`, `evidence.md` and the chat history out of every reviewer's input. They carry the author's conclusions, and a reviewer handed conclusions returns agreement.

Done when `diff.patch` exists, the base is named, and the standards list is written down.

## 2. Fan out the reviewers

This step is the review. Load the `ns-swarm` skill and launch each reviewer on the panel as its own fresh-context worker: subagents if the harness has any subagent or task tool, else `ns ask`. Reviewing the diff yourself in this context is not a review: you wrote or watched the change, so you share its blind spots. Inline review is allowed only when the harness has neither subagents nor `ns`, and then the gate below can't pass under `gates: auto`.

Use the **partition** shape, one worker per reviewer on the panel. Pick the panel from step 1's change size:

- **Reduced panel** when the diff is small: under about 50 changed lines, or it changes only tests, or only documentation that is not agent-facing instructions (nothing under `skills/`, no `AGENTS.md` or `CLAUDE.md`, nothing under `docs/agents/`). It runs correctness, spec and tests. Add security when the diff touches auth, secrets, input parsing or the merge path.
- **Full panel** otherwise: all eight reviewers below.

| Reviewer | Reference | `ns ask` role |
|---|---|---|
| correctness | [correctness.md](references/correctness.md) | `review.correctness` |
| readability | [readability.md](references/readability.md) | `review.readability` |
| architecture | [architecture.md](references/architecture.md) | `review.architecture` |
| security | [security.md](references/security.md) | `review.security` |
| performance | [performance.md](references/performance.md) | `review.performance` |
| tests | [tests.md](references/tests.md) | `review.tests` |
| spec | [spec.md](references/spec.md) | `review.spec` |
| comments | [comments.md](references/comments.md) | `review.comments` |

Roles let the user put different providers on different axes in `~/.config/nightshift/config.toml`. Run `ns doctor` once and note which provider serves each role, for the agreement ranking in step 3. Use `review` as the fallback role when an axis role is unconfigured.

Review passes are numbered from 0: pass 0 is the first review and pass n follows fix cycle n. Briefs go under `review-<pass>`, so this first pass uses `review-0`. `cycles:` in `review.md` is the number of fix cycles run. Build each worker's brief from [reviewer-brief.md](references/reviewer-brief.md): its one reference file, its axis's section of the [quality bar](../ns-contract/references/quality-bar.md), the paths to `diff.patch` and `brief.md`, the standards list, and, on pass 0 past the soft limit, the Split section. The quality bar is the single list of what must hold; `ns-build` checks its own diff against the same list. Every reviewer stays inside its own axis.

**Mutation check.** `ns ask` workers are read-only, so the correctness reviewer proposes mutations and you run them, up to five, exactly as the Mutation check in [correctness.md](references/correctness.md) says: only the tests covering the mutated file, each under a timeout. Record each command and result as evidence.

Done when every reviewer on the panel has a result or a gap entry. A gap is rerun once inline. A slice still missing after that makes the review `blocked`, never `pass`.

## 3. Synthesize

1. **Parse** every finding into: severity, `file:line`, one-line issue, evidence, raising reviewers (its axes) and their providers. Past the soft limit, also gather every reviewer's `SPLIT:` line into step 1's size Suggestion.
2. **Dedupe.** Merge findings that describe the same defect in different words. Keep every raiser.
3. **Rank by agreement.** Raised by workers on two or more providers ranks highest. Raised by two or more reviewers on one provider ranks next. A lone finding ranks last, except that lone security and correctness findings still get your full scrutiny.
4. **Judge** each finding against the diff text, as in [review-md.md](references/review-md.md). Trace the call path before accepting a hypothetical ("what if this is None?"). Dismiss "I would have done it differently" unless it names a concrete problem.
5. **Normalise severity** to Critical / Important / Suggestion with the mapping in [review-md.md](references/review-md.md).
6. **Scope** each finding, from the diff and the code at its location:
   - **Changed code**: the diff caused it. Its location is a line the diff adds, or the defect comes from a line the diff removes or a behaviour it changes (a dropped guard, a broken caller), or it is something the diff leaves out (a missing test, an unmet acceptance criterion).
   - **Pre-existing code**: the defect was there before the diff, unchanged by it. The unit didn't cause it and doesn't own that code, so it never blocks the unit and never goes to the fix loop. It is an **escape**: an earlier review missed it.
7. **File each escape** you didn't dismiss, whatever its severity, in the tracker per `docs/agents/issue-tracker.md`. Run its duplicate search first; when an open issue already covers the defect, use that issue instead of filing a new one. A new issue gets:
   - a body with the finding, its location, severity and evidence, a link to the unit's issue (or the unit ID when it has none), and the sentence "This escaped an earlier review."
   - the `needs-triage` state, a category label (`bug` for a defect), and an area label when `docs/agents/triage-labels.md` lists areas, using the label strings that file names
   - a priority role from severity: Critical, or any finding with security among its axes, gets `critical` (`high` when `triage-labels.md` has no `critical` row); Important gets `high`; Suggestion gets `low`

   Then set the finding's status to `deferred: #<n>`. An escape a later pass raises again keeps its ID and its `deferred` status. If the tracker can't be reached, leave the finding `open` and name it under Gaps in `review.md`; it still doesn't block.

Done when every raw finding is merged, ranked and carries a severity, scope, bucket and status, and every escape you kept is `deferred` to an issue or named under Gaps.

## 4. Write review.md

Write `.ns/<unit-id>/review.md` in the format in [review-md.md](references/review-md.md). Every finding has a severity, a `file:line`, its axes, its scope, the pass that raised it, evidence, its raisers, and a status: `open`, `fixed`, `dismissed: <reason>`, or `deferred: #<n>` for a filed escape.

## 5. Fix loop

While any Critical or Important finding in changed code is `open`, and fewer than 3 cycles have run:

1. Hand every open Critical and Important finding in changed code, on every axis, to a **fresh-context build agent**, launched in the foreground so you wait for it (see `ns-swarm`), working in the unit's worktree. Its brief: `brief.md`, `review.md`, the finding IDs to fix, and `docs/agents/stack.md`. It fixes behaviour test-first with the `ns-tdd` skill, routes comment findings through the `ns-no-comments` skill, runs the test command, and commits. It writes code, so it runs as a subagent or inline, never through read-only `ns ask`.
2. Regenerate `diff.patch` and rerun step 2 on the new diff as a later pass, briefed under `review-<n>` where `n` is this cycle. Reviewers get no list of earlier findings. The panel is correctness plus every reviewer that raised a changed-code finding you didn't dismiss in the previous pass. The size rule is not applied again.
3. Mark an earlier finding `fixed` only when the code at its location changed and no reviewer raised it again. Read the code to confirm. Synthesize new findings as in step 3, scoping each and filing new escapes, and record the cycle number as their `Cycle`.
4. Record the cycle's findings in `.ns/<unit-id>/review/cycle-<n>.md`, in the finding format of [review-md.md](references/review-md.md), and increment the cycle count there. Write `review.md` only when the loop ends (see the contract: an artifact is written once, at the end).
5. If the cycle added commits, re-run `ns-verify` before closing the loop, so `evidence.md` and `review.md` both carry the new HEAD `sha`. `ns-ship` rejects stale artifacts.

Architecture, readability and comments findings are fixed like correctness ones: no axis is deferred because of its kind. Suggestions and escapes never start a cycle and never go to the build agent.

A cycle where reviewers raised substantive findings and you dismissed all of them is a warning sign: you are validating, not reviewing. Say so in `review.md`.

After 3 cycles, stop the loop. An open Critical or Important in changed code (scope as in step 3) means the unit is below the bar and three cycles couldn't lift it. Merging it would compound the debt, and another build attempt would only repeat the loop, so it needs a human decision: `review.md` is `blocked`. The body's first line names every such finding (format in [review-md.md](references/review-md.md)). Below it, list the options:
- fix it by hand on the unit's branch, then delete `.ns/<unit>/review.md` so `ns run` reviews again (it stops on any `blocked` review.md until then)
- split the issue into smaller units
- route it to `ns-define` when the brief itself is wrong

## Gate

- `pass`: every reviewer slice on the panel has a result, every Critical and Important finding in changed code is `fixed` or `dismissed` with a reason, and every finding in pre-existing code is `dismissed` or `deferred` to an issue (or named under Gaps when the tracker couldn't take it). Escapes never hold a unit back, whatever their severity.
- `fail`: the review ended on something a build attempt can fix, and nothing is left that makes it `blocked` after the third cycle. For example, the fix cycles' commits leave the test command or the `ns-verify` re-run failing. An open Critical or Important in changed code after the third cycle is always `blocked`, even when another failure exists. The body's first line says what build must fix; `ns run` sends the body to build as feedback.
- `blocked`: a human decision is needed: an open Critical or Important in changed code remains after the third fix cycle (the "After 3 cycles" paragraph above), a reviewer slice could not run, the diff is empty, `brief.md` is missing, or (under `gates: auto`) the reviewers ran inline rather than in fresh contexts. The body's first line is the one-sentence reason `ns watch` quotes; [review-md.md](references/review-md.md) has the format. Record the launch mode in `review.md` (`launch: subagents | ns ask | inline`).

Under `gates: stop`, report `review.md` and wait. Under `gates: auto`, continue on `pass`. Next phase: load the `ns-ship` skill.
