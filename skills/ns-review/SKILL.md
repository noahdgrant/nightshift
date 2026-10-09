---
name: ns-review
description: REVIEW phase. Runs a local, multi-provider review of a unit's diff before push, writes review.md with Critical/Important/Suggestion findings, and loops fixes until no Critical stays open. Use after verify, before shipping, or when asked to review a branch or diff.
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
5. **Change sizing.** Count changed lines. Around 100 is good, 300 is fine for one logical change, around 1000 is too large: record an Important finding asking for a split (stack, by file group, horizontal or vertical slices). Also flag a refactor mixed with new behaviour, and any file the diff pushes past roughly 1000 total lines.

Leave `build.md`, `evidence.md` and the chat history out of every reviewer's input. They carry the author's conclusions, and a reviewer handed conclusions returns agreement.

Done when `diff.patch` exists, the base is named, and the standards list is written down.

## 2. Fan out the reviewers

This step is the review. Load the `ns-swarm` skill and launch every reviewer below as its own fresh-context worker: subagents if the harness has any subagent or task tool, else `ns ask`. Reviewing the diff yourself in this context is not a review: you wrote or watched the change, so you share its blind spots. Inline review is allowed only when the harness has neither subagents nor `ns`, and then the gate below can't pass under `gates: auto`.

Use the **partition** shape, one worker per reviewer:

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

Build each worker's brief from [reviewer-brief.md](references/reviewer-brief.md): its one reference file, the paths to `diff.patch` and `brief.md`, and the standards list. Every reviewer stays inside its own axis.

**Mutation check.** `ns ask` workers are read-only, so the correctness reviewer proposes mutations and you run them. For each one (up to five), in the worktree: copy the file, apply the mutation, run the test command, restore the file from the copy, and confirm `git status` matches what it was before. A mutation the suite survives is an Important finding naming the missing test case. Record the command and result as evidence.

Done when every reviewer has a result or a gap entry. A gap is rerun once inline. A slice still missing after that makes the review `blocked`, never `pass`.

## 3. Synthesize

1. **Parse** every finding into: severity, `file:line`, one-line issue, evidence, raising reviewers and their providers.
2. **Dedupe.** Merge findings that describe the same defect in different words. Keep every raiser.
3. **Rank by agreement.** Raised by workers on two or more providers ranks highest. Raised by two or more reviewers on one provider ranks next. A lone finding ranks last, except that lone security and correctness findings still get your full scrutiny.
4. **Judge** each finding against the diff text, as in [review-md.md](references/review-md.md). Trace the call path before accepting a hypothetical ("what if this is None?"). Dismiss "I would have done it differently" unless it names a concrete problem.
5. **Normalise severity** to Critical / Important / Suggestion with the mapping in [review-md.md](references/review-md.md).

Done when every raw finding is merged, ranked and carries a severity, bucket and status.

## 4. Write review.md

Write `.ns/<unit-id>/review.md` in the format in [review-md.md](references/review-md.md). Every finding has a severity, a `file:line`, evidence, its raisers, and a status: `open`, `fixed`, or `dismissed: <reason>`.

## 5. Fix loop

While any Critical or Important finding is `open`, and fewer than 3 cycles have run:

1. Hand the open Critical and Important findings to a **fresh-context build agent**, launched in the foreground so you wait for it (see `ns-swarm`), working in the unit's worktree. Its brief: `brief.md`, `review.md`, the finding IDs to fix, and `docs/agents/stack.md`. It fixes behaviour test-first with the `ns-tdd` skill, routes comment findings through the `ns-no-comments` skill, runs the test command, and commits. It writes code, so it runs as a subagent or inline, never through read-only `ns ask`.
2. Regenerate `diff.patch` and rerun step 2 for every reviewer on the new diff. Reviewers get no list of earlier findings.
3. Mark an earlier finding `fixed` only when the code at its location changed and no reviewer raised it again. Read the code to confirm. Add new findings with the cycle number.
4. Rewrite `review.md` and increment `cycles`.
5. If the cycle added commits, re-run `ns-verify` before closing the loop, so `evidence.md` and `review.md` both carry the new HEAD `sha`. `ns-ship` rejects stale artifacts.

A cycle where reviewers raised substantive findings and you dismissed all of them is a warning sign: you are validating, not reviewing. Say so in `review.md`.

After 3 cycles, stop the loop. Open Critical findings then mean the artifact is not ready: it needs a human, a split, or a return to `ns-plan`.

Open Important findings left after the loop become **follow-ups**, so they're tracked and don't stop the unit. File each one in the tracker per `docs/agents/issue-tracker.md`:
- the finding as an issue body, with the evidence
- `type:` and `area:` labels
- a link to the unit's issue

Then set the finding's status in `review.md` to `deferred: #<n>`. Suggestions stay in `review.md` only.

## Gate

- `pass`: no open Critical finding, every reviewer slice has a result, and every Important finding is `fixed`, `dismissed` with a reason, or `deferred` to a follow-up issue.
- `fail`: an open Critical finding remains after the fix loop. The body names each one.
- `blocked`: a reviewer slice could not run, the diff is empty, `brief.md` is missing, or (under `gates: auto`) the reviewers ran inline rather than in fresh contexts. Record the launch mode in `review.md` (`launch: subagents | ns ask | inline`).

Under `gates: stop`, report `review.md` and wait. Under `gates: auto`, continue on `pass`. Next phase: load the `ns-ship` skill.
