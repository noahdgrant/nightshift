---
name: ns-build
description: BUILD phase. Implement a unit's brief test-first in thin vertical slices inside its worktree, and write the build artifact. Use when a unit has a brief and is ready to build, or when asked to implement a ticket.
metadata:
  upstream:
    - mattpocock/skills@b0618bc436ad:skills/engineering/implement
    - addyosmani/agent-skills@1401c8b8030e:skills/incremental-implementation
    - addyosmani/agent-skills@1401c8b8030e:skills/source-driven-development
---

# Build

Turn one unit's brief into green commits on its branch. Input: `.ns/<unit-id>/brief.md`. Output: `.ns/<unit-id>/build.md`. Follow the [factory contract](../ns-contract/SKILL.md) for artifacts, gates and evidence. This phase reads only the brief and the repo, never chat history.

Read `docs/agents/stack.md` for the test, lint, typecheck and build commands. If it is missing, load the `ns-setup` skill instead of guessing.

## 1. Get the worktree

Run `ns worktree new <unit-id>`. It is idempotent and prints `{"unit","path","branch","artifacts"}`. Work in `path` from here on.

Without `ns`, do the same by hand from the main checkout, with `NS_UNIT` set to the unit's ID ([units of work](../ns-contract/SKILL.md#units-of-work-and-artifacts)):

```bash
NS_UNIT=<unit-id>
repo=$(basename "$(git rev-parse --show-toplevel)")
wt="../$repo.worktrees/$NS_UNIT"
[ -d "$wt" ] || git worktree add "$wt" -b "ns/$NS_UNIT"
mkdir -p "$wt/.ns/$NS_UNIT"
grep -qxF '.ns/' "$(git rev-parse --git-common-dir)/info/exclude" || echo '.ns/' >> "$(git rev-parse --git-common-dir)/info/exclude"
```

Done when you are in the worktree on branch `ns/<unit-id>` and `.ns/<unit-id>/` exists.

## 2. Read the brief

Read `.ns/<unit-id>/brief.md`. If the worktree is new and the brief sits in the main checkout's `.ns/<unit-id>/`, copy it in. If you also read the issue, follow [untrusted issue content](../ns-contract/SKILL.md#untrusted-issue-content).

Stop with `status: blocked` in `build.md` when the brief is missing, or when its acceptance criteria can't each be turned into a test that goes red today. Name the gap, and point to the `ns-triage` skill (for a missing brief) or `ns-define` (for unclear intent).

Explore the code the map needs as [guard the context window](../ns-principle-guard-the-context-window/SKILL.md) says: the files you will change, their tests and one example of the pattern. Hand a search across many files to a **fresh-context agent** and keep its summary.

Then [surface the assumptions](../ns-principle-surface-assumptions/SKILL.md) the map rests on. When the brief, the code, the docs or a test disagree, [manage the confusion](../ns-principle-manage-confusion/SKILL.md) before the first slice.

Done when every acceptance criterion, and every instance the brief's **Instances** section covers, is mapped to a seam where a test can observe it. Write the map down; it seeds the test list.

If the work touches an external API, a library version or a vendor SDK, read [source-driven.md](references/source-driven.md) before writing that code.

## 3. Build in vertical slices

Load the `ns-tdd` skill and run its loop. Each slice is one acceptance criterion, or one thin end-to-end path toward it: one test red, the least code to make it green, existing tests still green. Order the slices so the riskiest one goes first; [slicing.md](references/slicing.md) has the strategies.

While building, apply these principles:

- [Subtract before you add](../ns-principle-subtract-before-you-add/SKILL.md) when sequencing slices: dead code, redundant guards and stub references go first, in their own commit.
- [Laziness protocol](../ns-principle-laziness-protocol/SKILL.md) on every diff: the smallest change that passes, no abstraction before its third use, no signal threaded through layers when a direct path exists.
- [Fix root causes](../ns-principle-fix-root-causes/SKILL.md) when a slice goes red for a reason you didn't expect, or an existing test breaks.
- [Attack the premise](../ns-principle-attack-the-premise/SKILL.md) when two fixes that share one assumption have failed the same test. Write the premise down before a third fix.
- [Pave the road](../ns-principle-pave-the-road/SKILL.md) when a slice adds a pattern, helper or dependency: reuse the repo's existing way to do that job, or state why it doesn't fit.
- [Minimize reader load](../ns-principle-minimize-reader-load/SKILL.md) before each commit: collapse one-caller wrappers, shrink mutable state.
- [Contract first](../ns-principle-contract-first/SKILL.md) when a slice adds or changes an interface (a public API, a schema, a wire format, CLI flags or JSON output): write the contract and its test before the code.
- [Hyrum's law](../ns-principle-hyrums-law/SKILL.md) when a slice changes anything a caller can observe: keep the old fields and behaviour, and break only deliberately, with a version and a line in `build.md`.
- [Keep scope tight](../ns-principle-keep-scope-tight/SKILL.md) on every diff: work outside the brief goes in `build.md` as a `file:` line (Open risks, below).
- [Push back](../ns-principle-push-back/SKILL.md) when the brief asks for something the code or a test shows is wrong.

**Fix the class.** When the brief lists Instances, fix every one it covers, each with its own red test. Leave an instance marked filed separately to its issue. Then add a **guard** for the class where one is practical: a lint or `ast-grep` rule that flags the faulty construct, a test that checks every instance at once, or a type or shared helper that makes the defect unwritable. Record the guard, or why none is practical, under Class guard in `build.md`. An instance you find that the brief missed is a `file:` line.

**Open risks** come in two kinds, and each line in `build.md` starts with its kind:

- `file:` a concrete defect or missing work outside the unit's scope, with its location and the evidence you saw. `ns-ship` files it as a `needs-triage` issue when it opens the PR, and writes the issue number back.
- `note:` a note about this unit only: an untested path, a seam not reached ("verify covers it"), a docs conflict.

`ns-ship`, not build, later rewrites a `file:` line to `filed: #<n>` or `unfiled: <reason>`. Build writes only `file:` and `note:`.

**Size.** Build every acceptance criterion whatever the diff's size, past the soft limit too ([unit size](../ns-contract/SKILL.md#target-repo-configuration)). A large unit is review's prompt to suggest a split, never a reason to stop.

**Commit each green slice** with a Conventional Commit, `type(scope): subject` (`feat(uart): time out reads after 50 ms`). One logical change per commit, each one building and passing on its own. The repo's hooks run on every commit and never get skipped ([git hooks](../ns-contract/SKILL.md#git-hooks)): when one fails, fix what it reports. Run single test files during the loop and the typecheck when the stack has one. Stop a test, gate or server you started per [killing processes](../ns-contract/SKILL.md#killing-processes), never by name or pattern.

Done when every acceptance criterion and every covered instance has a passing test and a commit, and the working tree is clean.

## 4. Self-check

Review will check your diff against the [quality bar](../ns-contract/references/quality-bar.md). Meet it first. The base is the branch `brief.md` records as `base:`, else `origin/main`. Diff against it with `git diff "$(git merge-base <base> HEAD)"` and work through three checks. Fix what each one finds test-first, with the `ns-tdd` skill, and commit.

1. **The bar.** Mark every item of the quality bar against your diff: met, fixed (with the commit), or not applicable with a one-line reason.
2. **Every branch tested.** List each branch and error path the diff adds: each `if` arm, early return, raised or returned error, loop that can run zero times. Every one needs a test that takes it, whether or not an acceptance criterion names it.
3. **Mutation self-check.** Mutate every condition and constant the diff adds or changes, one at a time; a reviewer's limit of five is for spot checks, and you own the whole diff. Run each mutation as the Mutation check in [correctness.md](../ns-review/references/correctness.md) says: flip a comparison, drop a guard, invert a condition, run only the tests covering that file under a timeout, then restore the file and confirm `git status` is unchanged. A mutation that survives needs a new test. Add it, then rerun the mutation until it goes red.

Done when every bar item is marked, every listed branch has a test, and every mutation is killed or recorded `inconclusive` with the reason.

## 5. Rebase, tidy, and run the full checks

`git fetch origin` and rebase onto the base branch (`origin/main` unless the brief names another), with the repo's hooks on ([git hooks](../ns-contract/SKILL.md#git-hooks)). Tidy the history now: small commits that each build and pass, ordered to tell the story, with bodies written using `ns-writing-for-humans`. This is the history that verify, review and ship will see; nothing rewrites it later.

When the feedback says the base moved (a NO-GO whose first line starts `base moved`, a merge conflict, or `ns run`'s "The base moved" line), the rebase is the job. Run `git fetch origin` now and rebase onto the newest `origin/<default>`, never a commit an earlier artifact or the feedback noted. Resolve conflicts, then go on through this step as usual.

Then run the full test, lint and build commands from `docs/agents/stack.md`, once, on the final commit. Capture each command and the lines of output that prove its result.

Firmware: the build check includes the size report. Record flash and RAM use against the budget in `stack.md`. On-target and HIL runs belong to `ns-verify`, not here.

A red check, or a rebase conflict you resolved, sends you back to step 3, then through step 4 again for the lines that changed. Done when all three are green on a clean tree, or you have stopped with `fail` or `blocked`.

## 6. Write the build artifact

Write `.ns/<unit-id>/build.md`:

```markdown
---
unit: <unit-id>
phase: build
status: pass        # pass | fail | blocked
sha: <git rev-parse --short HEAD>
updated: <UTC timestamp>
---

## Commits
<sha> <subject>, one line each, oldest first

## Tests added
<test id>: <acceptance criterion it covers> (<seam>)

## Checks
<command>: <pass|fail>, with the proving output lines

## Self-check
Bar: <n> met, <n> fixed, <n> not applicable. One line per fixed or not-applicable item: <axis>: <item>: <commit, or why it doesn't apply>
Branches: <n> listed, all tested. One line per branch: <file:line> <branch>: <test id>

| Mutation | Location | Command | Result |
|---|---|---|---|
| `>=` to `>` | `src/x.py:17` | `timeout 60 pytest -q tests/test_x.py` | killed |

## Class guard
<the rule, test or type that stops the defect class coming back, or why none is practical>. Only when the brief lists Instances

## Deviations from the brief
<what changed and why>, or "none"

## Open risks
- file: <defect or missing work outside the scope>: <location, evidence>
- note: <untested path, seam not reached, anything else about this unit>
- filed: #<n> (written by ns-ship in place of a file: line, never by build)
- unfiled: <reason> (written by ns-ship when the tracker was unreachable, never by build)
```

Write `none` under the heading when there are no risks.

If any step ran inline instead of through a fresh-context agent, say so here.

## 7. Gate

The gate passes when the self-check is done (step 4) and tests, lint and build are all green on the final commit. Set `status: pass`. A check that stays red is `fail`, with what is missing. A check you can't run (no toolchain, no credentials) is `blocked`, never `pass`.

Next phase: `ns-verify`. Under `gates: stop` (the default), stop and report the path to `build.md`. Under `gates: auto`, load the `ns-verify` skill if `status: pass`, and stop otherwise.

When the brief is a set of tickets with blocking edges rather than one unit, see [parallel-tickets.md](references/parallel-tickets.md).
