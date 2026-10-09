---
name: ns-auto
description: Drive one unit from issue to merge-ready PR in this session, chaining triage, build, verify, review and ship under a stated gate policy. Starts by agreeing an overnight contract.
disable-model-invocation: true
metadata:
  upstream: cursor/plugins@ccb5507cec15:pstack/skills/poteto-mode/playbooks/autonomous-run.md
---

# Auto

You are the **foreman** for one unit. You agree the contract, then pick the next phase from the unit's artifacts, run it, read its gate, and repeat until the unit is done or stuck. The phases do the work. You own the exit condition and the hand-back.

Follow the [factory contract](../ns-contract/SKILL.md) for units, worktrees, artifacts, frontmatter and gates.

## 1. Agree the overnight contract

Before any phase runs, every term below has a value. Take the terms the prompt already states, and ask for the rest in one message.

| Term | Holds | Default |
|---|---|---|
| Goal | the issue or brief this run delivers | none: without a goal, stop and ask |
| Done | the checkable condition that ends the run | `evidence.md`, `review.md` and `pr.md` pass at HEAD |
| Unit and worktree | unit ID, worktree path, base branch | `<issue>-<slug>`, the path `ns worktree new` prints, `main` |
| Allowed actions | the gate policy, and whether the run may push `ns/<unit-id>`, open a PR and comment on the tracker | `gates: stop`, no push |
| Escape hatch | when to stop early and hand back: a deadline, a budget, a phase to stop after, where the report goes | stop at the first stuck unit, report in this session |

When the prompt says nobody will answer (an overnight or headless run), fill each missing term with its default instead of asking, except the goal. Apply [never block on the human](../ns-principle-never-block-on-the-human/SKILL.md) there.

Some actions wait for a human whatever the contract grants: merging, approving, pushing to the default branch, and the rest of the contract's always-human list. A run that may not push ends before `ns-ship`, which pushes and opens the PR. That is an escape-hatch stop, reported as such.

Say the gate policy in your first message: `Running <unit-id> under gates: <policy>.`

Done when all five terms have a value and you have stated the policy.

## 2. Pick the runner

Defer to `ns run` when both hold:

- `command -v ns` finds the CLI.
- A factory definition exists: `.nightshift/nightshift.toml` in the main worktree's root, or `nightshift.toml` at the root of a factory repo.

Then run one of these and wait for it:

```bash
ns run --issue <n> --gates <policy>        # unit not started yet
ns run <unit-id> --gates <policy>          # unit already has a worktree
```

`ns run` enforces gates, retries, runner locks, the merge rules and the human-only actions in code. Leave the loop to it. When it exits, report its final JSON (`outcome`, `phase`, `reason`, `pr`, `artifact`) and go to step 4. Exit 5 means another run holds the repo's lock: report that and stop. A deadline in the escape hatch is yours to watch. When it passes, stop the `ns run` you started by its PID, as the contract's rules for killing processes say, and report the phase it was in.

Otherwise run the loop yourself, step 3.

## 3. Run the loop in this session

Create or reuse the worktree with `ns worktree new <unit-id>`, or by hand as the contract describes, and work there. Start the run log `.ns/<unit-id>/auto/log.md` with the contract's five terms. Record `git ls-remote origin refs/heads/<base>` when the repo has an `origin`.

Each turn:

1. Read the frontmatter of every artifact in `.ns/<unit-id>/`, skipping `history/`. An artifact is **current** when its `sha` matches `git rev-parse --short HEAD` by prefix.
2. Pick the next step from the first row that matches:

   | State | Next |
   |---|---|
   | any artifact `blocked` | stuck |
   | the contract's done condition holds | done |
   | no `brief.md` | triage, when the goal names an issue. Otherwise stuck, "no brief" |
   | `brief.md` not `pass` | stuck: triage routed it to a human or `ns-define` |
   | no `build.md`, or build `fail` | build |
   | no `evidence.md`, or evidence not current | verify |
   | evidence `fail` | build |
   | no `review.md`, or review not current | review |
   | review `fail` | build |
   | no `pr.md`, or pr `pass` and not current | ship |
   | pr `fail` | the phase its body names first (verify or review), else stuck |
   | the phase has used its attempts | stuck |

   Each phase gets two attempts per run unless the contract says otherwise.
3. Archive before you run phase P. Move P's artifact, and every downstream artifact that is not both `pass` and current, to `.ns/<unit-id>/history/<artifact>-<n>.md`, with n one past the highest already there. The order is brief, build, evidence, review, pr. What stays in `.ns/<unit-id>/` is then current.
4. Run P. Load the `ns-<P>` skill with this prompt, adding the feedback line when a `fail` sent the work back:

   ```
   Run the ns-<P> skill for unit <unit-id> (issue <url or path>). Work in <worktree>. gates: <policy>.
   Phase: <P> (attempt <n>). End by writing the phase's artifact under .ns/<unit-id>/ with its frontmatter status, then stop. ns-auto picks the next phase.
   Feedback: <the body of the artifact that sent the work back>
   ```

   Hand build to a **fresh-context agent** when the harness has subagents, so the author is not in the context that verifies and reviews. Run the other phases in this session: verify and review launch their own fresh-context workers. With no subagents, build runs inline. Note that in the run log.
5. Read the result. If P wrote no artifact, the attempt failed with "no artifact written". Move the files you archived in substep 3 back, so the next turn sees the state as it was.

   Run the CI gate after a build that wrote `pass`, and after a review that moved HEAD. The command is the `ci-local` row of the Commands table in `docs/agents/stack.md`; with no such row, skip the gate. Run it in the worktree. A non-zero exit is red: the next step is build, using an attempt, with the command, its exit and the last 40 lines of its output as the feedback.
6. Check the human-only actions. If the base branch on `origin` moved to a commit reachable from the unit's HEAD, the run pushed to it: stuck, "default branch moved". If `pr.md` names a PR and `gh pr view <n> --json state` says `MERGED`, stuck, "PR merged by run". Report either one as the first line of your report.
7. Append one line to the run log: phase, attempt, status, sha, and the artifact body's first line.
8. Apply the gate policy. Under `gates: stop`, stop here and report. Invoking `ns-auto` again resumes from the table. Under `gates: auto`, take the next turn, and check the escape hatch first.

When two attempts that share one premise fail the same gate, apply [attack the premise](../ns-principle-attack-the-premise/SKILL.md) before the next one. For any other call that is yours (a contract default, a retry, a send-back), read the [principles index](../ns-principles/SKILL.md) and apply the principles whose trigger fits. Name each one in the run log with the choice it changed.

Done when the table says done or stuck, or the escape hatch fires.

## 4. Report

The unit ends at an open PR. A human merges it, or `ns run`'s merge step does under its own rules. Never merge, approve, or push to the default branch, even when asked to. Point the human at the PR instead.

Write the report with the `ns-writing-for-humans` skill:

- the done condition, and whether it holds, with the artifact that proves it
- the phases run, from the run log or `ns run`'s `phases`
- the PR URL, if there is one
- the artifact that stopped the run and its first body line, when it stopped short
- what waits on a human: the merge, an approval, a `blocked` artifact's blocker
