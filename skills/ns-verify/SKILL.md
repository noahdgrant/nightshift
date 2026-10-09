---
name: ns-verify
description: VERIFY phase. Prove a built unit meets every acceptance criterion by driving the real surface, and write .ns/<unit-id>/evidence.md. Use after ns-build passes, or when asked to verify, prove, or show evidence that a change works.
metadata:
  upstream:
    - cursor/plugins@ccb5507cec15:pstack/skills/principle-prove-it-works
    - cursor/plugins@ccb5507cec15:pstack/skills/benchmark-checklist
    - cursor/plugins@ccb5507cec15:pstack/skills/poteto-mode/playbooks/bug-fix.md
    - cursor/plugins@ccb5507cec15:pstack/skills/poteto-mode/playbooks/feature.md
    - cursor/plugins@ccb5507cec15:pstack/docs/guide/06-verify-and-ship.md
---

# Verify

"It compiles" is not evidence, and neither is "the tests pass". This phase drives the real surface for each acceptance criterion and records proof a reviewer can re-run. It applies [prove it works](../ns-principle-prove-it-works/SKILL.md) to every criterion: check the real artifact, never a proxy or a self-report.

Input: `.ns/<unit-id>/brief.md` and `build.md`. Output: `.ns/<unit-id>/evidence.md`. Work in the unit's worktree (`ns worktree list`, or `git worktree list` without `ns`).

## 1. Read the unit

Read `brief.md` and `build.md`. Use only these and the repo, never chat history.

- If `build.md` is missing or its status is not `pass`, stop. Verification starts after the build gate.
- Note `build.md`'s Open risks and Deviations. Untested paths and unreached seams are where the surface check matters most.
- Number the acceptance criteria `AC1..ACn` in the brief's order. For a bug, the brief's repro is a criterion of its own: it must fail on the base and pass on the branch.
- A criterion with no observable end state can't be verified. Record it as INCONCLUSIVE with "criterion not checkable" and carry on with the rest.

Done when every criterion has an ID and a sentence naming the observable state that would prove it.

## 2. Find the verification lever

Read `docs/agents/verify.md`, and `docs/agents/stack.md` beside it. If either is missing, load the `ns-setup` skill instead of guessing.

- **Verify skill and control CLI exist**: load the project's `verify-<project>` skill named there, and run its Doctor. Fix or report a failing doctor before driving anything.
- **No verify skill**: load the `ns-setup-verify` skill, finish it, then return here.
- **No verify skill, and the run is time-boxed** (an overnight contract's deadline, or the human said so): verify with the build and test commands in `stack.md` instead, and record `Strength: stack-only` in the evidence. Unit tests show branch behavior, not that the user-facing behavior works. A criterion about runtime behavior that only the real surface can show is INCONCLUSIVE under this fallback.

Prefer a judge who is not the author. If this context also ran `ns-build`, hand steps 3 to 6 to a **fresh-context agent** (role `verify`) with the unit ID and worktree path. If none is available, verify inline and record `Verified by: inline` in the evidence.

## 3. Drive each criterion on its matching surface

Map each criterion to the feature-map entry that covers it, then drive that entry through the verify skill and control CLI. Pass `--out .ns/<unit-id>/evidence/` so artifacts land with the unit. Match the check to the change:

- A CLI change runs the real command.
- A service change sends the real request and reads the result back through a second view.
- A parser or migration replays a saved real input.
- A storage change reads back the written value.
- A performance change compares before and after (step 5).
- A firmware change runs on the bench: flash, reset, drive over serial, capture. An emulator proves logic only. Anything about timing, peripherals or power needs hardware, or the criterion is INCONCLUSIVE.
- A bug fix reruns the original repro on the same surface. Show it failing on the base commit (check it out in a scratch worktree with `git worktree add --detach <path> <base>`) and passing on the branch.

For each criterion, capture:

- the command
- an output excerpt that shows the end state
- the artifact path (log, transcript, capture, response body)

When a check fails, suspect the observation first: re-check how you looked before you call the system broken. When the evidence is a test, it must fail if the behavior broke. Check it against [test behavior, not implementation](../ns-principle-test-behavior-not-implementation/SKILL.md).

Verdicts:

- **PASS**: the criterion's end state was observed on the matching surface.
- **FAIL**: a different end state was observed.
- **INCONCLUSIVE**: the check couldn't run, ran on the wrong surface, or the evidence doesn't show the end state. Before settling here, try to make it checkable: extend the control CLI or feature map, or synthesize the trigger.

Run the verify skill's Cleanup when done, then confirm the artifacts still exist. Anything you started outside it, stop per [killing processes](../ns-contract/SKILL.md#killing-processes).

Done when every criterion has a verdict, and every PASS and FAIL has a command, an excerpt and an artifact path.

## 4. Fan out a large surface

When the criteria span several feature-map entries, or one entry is large, load the `ns-swarm` skill with one worker per feature-map entry. Each worker brief carries the unit ID, the exact commit SHA, its criterion IDs, the feature file path, and its own `--out .ns/<unit-id>/evidence/<feature>/`. Workers run with role `verify` in write mode (`ns ask --role verify --write --cwd <worktree>`), because they execute commands.

Map worker reports to verdicts: PASS to PASS, ISSUES to FAIL, BLOCKED to INCONCLUSIVE. A missing or SHA-less result is INCONCLUSIVE. Workers that share one bench or one instance run one at a time.

## 5. Vet every measured number

Any number in the evidence (latency, throughput, size, current draw, ISR time) goes through [benchmark-checklist.md](references/benchmark-checklist.md) before it counts. It is the working form of [explain the number](../ns-principle-explain-the-number/SKILL.md). A number whose checklist verdict is inconclusive makes its criterion INCONCLUSIVE.

## 6. Write evidence.md

Write `.ns/<unit-id>/evidence.md`:

````markdown
---
unit: <unit-id>
phase: verify
status: pass | fail | blocked
sha: <git rev-parse --short HEAD>
updated: <UTC timestamp>
---

# Evidence: <unit-id>

Commit: <sha> · Strength: surface | stack-only · Verified by: fresh-context agent | inline
Verify skill: <path> · Doctor: pass

| ID | Criterion | Verdict | Surface |
|---|---|---|---|
| AC1 | <criterion> | PASS | <bench, CLI, HTTP, emulator, tests> |

## AC1: <criterion>

- Command: `<exact command>`
- Output:
  ```
  <excerpt showing the end state>
  ```
- Artifact: `.ns/<unit-id>/evidence/<file>`

## Numbers

<per number: verdict, value with unit, run count, range, limiter; or "none">

## Gaps

<per FAIL or INCONCLUSIVE: what was seen, and what would fix it or make it checkable; or "none">
````

Set `status` from the verdicts:

- `pass`: every criterion is PASS.
- `fail`: any criterion is FAIL. `ns-build` picks the unit up from the Gaps section.
- `blocked`: no FAIL, but at least one INCONCLUSIVE. Gaps names the blocker (a bench, credentials, a criterion the human must sharpen).

## Gate

The gate holds when `evidence.md` has `status: pass`. An INCONCLUSIVE never counts as a pass, and a stack-only PASS is reported as weaker evidence.

Under `gates: stop`, stop and report the status and the Gaps. Under `gates: auto`, a pass moves on to the `ns-review` skill, and anything else stops. With no policy given, use `stop`.
