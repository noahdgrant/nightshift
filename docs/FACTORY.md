# Factory definition and runner

The minimum slice of D15 to D17 (`docs/DESIGN.md`) that runs nightshift unattended: a definition in `.nightshift/`, `ns run` to drive one unit through the phases, and `ns watch` to pull units from the tracker overnight. Automations, scorers and runner kinds come later (#5, #7).

## Layout

```
.nightshift/
  nightshift.toml          the definition root
  agents/<role>/agent.md   one role per phase (optional: built-in default prompts exist)
```

The root is the directory holding `nightshift.toml`: `.nightshift/` in a product repo, or the repo root of a factory repo. `ns` finds it from `--factory <dir>`, else `<main-worktree-root>/.nightshift/`.

## nightshift.toml

```toml
name = "nightshift"
gates = "auto"                     # stop | auto. ns run passes it to every phase prompt

[worktree]
setup = ["git submodule update --init"]

[defaults]
harness = "claude"                 # a [harness.<name>] from the user config, or built-in claude/codex
model = "opus"
timeout_minutes = 45               # per phase run
max_attempts = 2                   # per phase, per unit

[phases.triage]
skill = "ns-triage"
[phases.build]
skill = "ns-build"
timeout_minutes = 90
[phases.verify]
skill = "ns-verify"
[phases.review]
skill = "ns-review"
[phases.ship]
skill = "ns-ship"

[queue]
source = "github"                  # github only, for now. Uses `gh`
ready_label = "status:ready-for-agent"
in_progress_label = "status:in-progress"
done_label = "status:in-review"    # set when the unit ends with an open PR
stuck_label = "status:ready-for-human"
order = ["type:fix", "type:feat", "type:refactor", "type:test", "type:docs", "type:chore"]

[limits]
max_units = 4                      # per `ns watch` invocation
budget_usd = 25.0                  # summed from harness cost reports; stop starting new phases past it
```

A phase table may override `harness`, `model`, `timeout_minutes` and `max_attempts`. Unknown keys are errors. `ns factory validate` checks the file.

## agents/<role>/agent.md

```markdown
---
role: build
---
Load the `{skill}` skill and run it for unit `{unit}` (issue {issue_url}).
Work in {worktree}. gates: {gates}.
```

Placeholders: `{skill}`, `{unit}`, `{issue}`, `{issue_url}`, `{worktree}`, `{gates}`, `{phase}`, `{attempt}`, `{feedback}` (the body of the artifact that sent work back, empty on a first attempt). Without a file, `ns run` uses a built-in prompt of this shape.

## ns run

```
ns run <unit-id> [--issue <n>] [--from <phase>] [--gates stop|auto] [--dry-run] [--factory <dir>]
ns run --issue <n>                 # unit id <n>-<slug from the issue title>
```

Steps:

1. Create or reuse the worktree (`ns worktree new`, including setup).
2. Read `.ns/<unit>/*.md` frontmatter and pick the next phase from the state table.
3. Run the phase's harness headless in write mode (`command_write`), with cwd set to the worktree and the rendered prompt on stdin. Enforce the timeout by killing the process group.
4. Re-read the phase's artifact. If it's missing or unchanged since before the run, the attempt failed with "no artifact written".
5. Repeat until the unit is done, stuck, or out of budget.

| Last state | Next |
|---|---|
| no `brief.md`, issue given | triage |
| no `brief.md`, no issue | stuck: "no brief" |
| `brief` pass, no `build` | build |
| `build` pass, no `evidence` or evidence `sha` ≠ HEAD | verify |
| `evidence` pass, no `review` or review `sha` ≠ HEAD | review |
| `review` pass, no `pr` | ship |
| `pr` pass | **done** |
| triage ended `blocked`, or its issue state isn't ready-for-agent | stuck (triage decided a human or define is needed) |
| `verify` fail, `review` fail | build again, with that artifact's body as `{feedback}` |
| `ship` fail | the phase its body names (verify or review), else stuck |
| any `blocked` | stuck |
| a phase out of attempts | stuck |

Hard rules, enforced in code whatever the prompts say:

- Never merge, never approve, and never push to the default branch. `ns run` checks after every phase that the default branch's remote ref hasn't moved because of this run, and that no PR it touched was merged. A breach stops the run and marks it stuck.
- One unit at a time per repo. A lock file in the common git dir (`ns-run.lock`, holding pid and unit) refuses a second runner.

Run log: append one JSON line per event to `.git/ns/runs.jsonl` in the common git dir: unit, phase, attempt, decision, artifact status, sha, cost, tokens, wall time, exit. `ns run --dry-run` prints the next decision and the rendered prompt, and runs nothing.

Output: final JSON `{unit, outcome: done|stuck|budget, phase, reason, pr, cost_usd, phases:[...]}`. Exit 0 for done, 1 for stuck, 3 for budget.

## ns watch

```
ns watch [--once] [--until HH:MM] [--max-units N] [--dry-run] [--factory <dir>]
```

1. List open issues with `ready_label` via `gh issue list --json number,title,labels,body`.
2. Drop issues whose first-line `Blocked by: #a, #b` names any open issue, and issues that already have an open PR whose body says `Closes #n`.
3. Sort by the first matching `order` label, then by issue number.
4. Take the first. Swap `ready_label` for `in_progress_label`, then `ns run --issue <n>`.
5. On `done`, swap to `done_label`. On `stuck`, swap to `stuck_label` and comment the reason and the last artifact path. The comment carries the AI disclaimer.
6. Repeat until the queue is empty, `--until` passes (no new unit starts after it), `max_units` is reached, or the budget is spent.

`--once` takes one unit. `--dry-run` prints the ordered queue with skip reasons. `gh` runs with whatever `GH_TOKEN` the environment carries, so the account is chosen by whoever starts `ns watch`.

## Starting a night

```bash
export GH_TOKEN=$(gh auth token --user <account>)   # the account PRs should come from
ns watch --until 06:30 >> ~/.local/share/nightshift/watch.log 2>&1
```

The phases run with permissions bypassed inside worktrees. Run on a machine where that's acceptable.
