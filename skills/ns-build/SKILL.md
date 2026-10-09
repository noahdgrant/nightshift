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

Without `ns`, do the same by hand from the main checkout:

```bash
repo=$(basename "$(git rev-parse --show-toplevel)")
wt="../$repo.worktrees/<unit-id>"
[ -d "$wt" ] || git worktree add "$wt" -b ns/<unit-id>
mkdir -p "$wt/.ns/<unit-id>"
grep -qxF '.ns/' "$(git rev-parse --git-common-dir)/info/exclude" || echo '.ns/' >> "$(git rev-parse --git-common-dir)/info/exclude"
```

Done when you are in the worktree on branch `ns/<unit-id>` and `.ns/<unit-id>/` exists.

## 2. Read the brief

Read `.ns/<unit-id>/brief.md`. If the worktree is new and the brief sits in the main checkout's `.ns/<unit-id>/`, copy it in. If you also read the issue, follow [untrusted issue content](../ns-contract/SKILL.md#untrusted-issue-content).

Stop with `status: blocked` in `build.md` when the brief is missing, or when its acceptance criteria can't each be turned into a test that goes red today. Name the gap, and point to the `ns-triage` skill (for a missing brief) or `ns-define` (for unclear intent).

Done when every acceptance criterion is mapped to a seam where a test can observe it. Write the map down; it seeds the test list.

If the work touches an external API, a library version or a vendor SDK, read [source-driven.md](references/source-driven.md) before writing that code.

## 3. Build in vertical slices

Load the `ns-tdd` skill and run its loop. Each slice is one acceptance criterion, or one thin end-to-end path toward it: one test red, the least code to make it green, existing tests still green. Order the slices so the riskiest one goes first; [slicing.md](references/slicing.md) has the strategies.

While building, apply these principles:

- [Subtract before you add](../ns-principle-subtract-before-you-add/SKILL.md) when sequencing slices: dead code, redundant guards and stub references go first, in their own commit.
- [Laziness protocol](../ns-principle-laziness-protocol/SKILL.md) on every diff: the smallest change that passes, no abstraction before its third use, no signal threaded through layers when a direct path exists.
- [Fix root causes](../ns-principle-fix-root-causes/SKILL.md) when a slice goes red for a reason you didn't expect, or an existing test breaks.
- [Attack the premise](../ns-principle-attack-the-premise/SKILL.md) when two fixes that share one assumption have failed the same test. Write the premise down before a third fix.
- [Minimize reader load](../ns-principle-minimize-reader-load/SKILL.md) before each commit: collapse one-caller wrappers, shrink mutable state.

**Scope.** Touch only what the brief requires. Something worth fixing outside it goes in `build.md` under open risks, not in the diff. A change the brief asked for that you made differently goes under deviations, with the reason.

**Size.** After each commit, check the unit's changed lines against the hard limit in `docs/agents/stack.md` ([unit size](../ns-contract/SKILL.md#target-repo-configuration)). Past it with acceptance criteria still unmet, stop: write `build.md` with `status: blocked` and "unit too large, split it" as the first line of the body, and list the criteria met and the ones left so triage can split the rest.

**Commit each green slice** with a Conventional Commit, `type(scope): subject` (`feat(uart): time out reads after 50 ms`). One logical change per commit, each one building and passing on its own. Run single test files during the loop and the typecheck when the stack has one.

Done when every acceptance criterion has a passing test and a commit, and the working tree is clean.

## 4. Rebase, tidy, and run the full checks

`git fetch origin` and rebase onto the base branch (`origin/main` unless the brief names another). Tidy the history now: small commits that each build and pass, ordered to tell the story, with bodies written using `ns-writing-for-humans`. This is the history that verify, review and ship will see; nothing rewrites it later.

Then run the full test, lint and build commands from `docs/agents/stack.md`, once, on the final commit. Capture each command and the lines of output that prove its result.

Firmware: the build check includes the size report. Record flash and RAM use against the budget in `stack.md`. On-target and HIL runs belong to `ns-verify`, not here.

A red check sends you back to step 3. Done when all three are green on a clean tree, or you have stopped with `fail` or `blocked`.

## 5. Write the build artifact

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

## Deviations from the brief
<what changed and why>, or "none"

## Open risks
<untested paths, seams not reached, things noticed but out of scope>, or "none"
```

If any step ran inline instead of through a fresh-context agent, say so here.

## 6. Gate

The gate passes when tests, lint and build are all green on the final commit. Set `status: pass`. A check that stays red is `fail`, with what is missing. A check you can't run (no toolchain, no credentials) is `blocked`, never `pass`. A unit past the hard limit is `blocked` (step 3).

Next phase: `ns-verify`. Under `gates: stop` (the default), stop and report the path to `build.md`. Under `gates: auto`, load the `ns-verify` skill if `status: pass`, and stop otherwise.

When the brief is a set of tickets with blocking edges rather than one unit, see [parallel-tickets.md](references/parallel-tickets.md).
