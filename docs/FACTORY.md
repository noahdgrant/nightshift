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
max_attempts = 2                   # per phase, per unit, per `ns run` invocation
billing = "subscription"           # subscription | api. See "Billing"

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
budget_usd = 25.0                  # soft cap, summed from harness cost reports. Unset: no cap
                                   # under billing = "subscription", 25.0 under "api"

[merge]                            # absent table: policy = "human"
policy = "auto"                    # auto | human. See "Merge"
protected = [".github/**", "scripts/check-private.sh", ".nightshift/**", "cli/src/run.rs"]
ci_timeout_minutes = 30
```

Every table and key is optional; the defaults are the values above, except `limits.budget_usd` and `merge` as noted. A phase table may override `harness`, `model`, `timeout_minutes` and `max_attempts`, and its `skill` defaults to `ns-<phase>`. Unknown keys and unknown phases are errors. `ns factory validate` checks the file and every `agents/<role>/agent.md` (role matches the directory, placeholders are known), and exits 1 on any problem.

## agents/<role>/agent.md

```markdown
---
role: build
---
Load the `{skill}` skill and run it for unit `{unit}` (issue {issue_url}).
Work in {worktree}. gates: {gates}.
```

Placeholders: `{skill}`, `{unit}`, `{issue}`, `{issue_url}`, `{worktree}`, `{gates}`, `{phase}`, `{attempt}`, `{feedback}` (the body of the artifact that sent work back, or the failing CI checks and log tail, empty on a first attempt). They are replaced in one pass, so placeholder-like text inside `{feedback}` stays literal. Without a file, `ns run` uses a built-in prompt of this shape, with a `Phase: <phase> (attempt <n>)` line and the feedback appended when there is any.

## ns run

```
ns run <unit-id> [--issue <n>] [--from <phase>] [--gates stop|auto] [--dry-run] [--factory <dir>]
ns run --issue <n>                 # unit id <n>-<slug from the issue title>
ns run <unit-id> --base origin/main   # base for a new worktree (ns watch passes it)
```

Steps:

1. Create or reuse the worktree (`ns worktree new`, including setup).
2. Read `.ns/<unit>/*.md` frontmatter and pick the next phase from the state table.
3. Run the phase's harness headless in write mode (`command_write`), with cwd set to the worktree and the rendered prompt on stdin. Enforce the timeout by killing the process group. The built-in `claude` write command for `ns run` is `claude -p --permission-mode bypassPermissions --model {model} --output-format stream-json --verbose`: the phases need Bash, which `acceptEdits` can't grant headless. A configured claude command gets `--output-format stream-json --verbose` added if it lacks them. Stdout goes to `<git-common-dir>/ns/transcripts/<unit>/<phase>-<attempt>.jsonl`, and cost and tokens are parsed from it.
4. Re-read the phase's artifact. If it's missing or unchanged since before the run (same mtime and content hash), the attempt failed with "no artifact written". A triage run that exits 0 without writing `brief.md` is the "triage decided a human or define is needed" row: stuck, no retry.
5. Repeat until the unit is done, merged, stuck, paused, or out of budget.

"`sha` ≠ HEAD" compares the frontmatter `sha` with `git rev-parse --short HEAD` in the worktree by prefix, since the lengths may differ. Rows are checked top to bottom, with these refinements, all by artifact mtime:

- `pr` pass is **done** only if `pr.md` is newer than `build.md`. Otherwise build ran again after the PR was shipped (a CI fix), and once verify and review hold at HEAD, ship runs again onto the same PR.
- A `verify` or `review` fail older than `build.md` means build already answered it: that phase runs again instead of build.
- A `ship` fail sends work to the phase its body names first; once that phase's artifact is newer than `pr.md`, ship runs again.
- A `build` fail retries build, with its own body as `{feedback}`.
- A review that commits leaves `evidence.md` stale, so verify runs again before ship. `review.md` itself carries the new sha and stays valid.

Attempts are counted per invocation, so re-running `ns run` on a stuck unit gives each phase fresh attempts.

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

- Phases never merge, never approve, and never push to the default branch. Only `ns run`'s own merge step merges (see "Merge").
- Before the first phase, `ns run` records `git ls-remote origin` for the default branch (skipped without an `origin` remote). After every phase it reads it again. If it moved to a commit reachable from the unit's HEAD, this run pushed to it: stuck with "default branch moved". A move to anything else is another actor (a human merge, another machine): it is logged as `default_moved_by_other_actor` and becomes the new baseline.
- After every phase, if `pr.md` has `pr:` (a number or a URL ending in one), `gh pr view <n> --json state`. `MERGED` means a phase merged it: stuck with "PR merged by run".
- One unit at a time per repo. A lock file in the common git dir (`ns-run.lock`, holding pid and unit) refuses a second runner with exit 5. A lock whose pid is dead is removed.

Run log: append one JSON line per event to `.git/ns/runs.jsonl` in the common git dir: unit, phase, attempt, decision, artifact status, sha, cost, tokens, wall time, exit. Start, end, breaches, CI failures and merges are events too. `ns run --dry-run` prints the next decision, the rendered prompt and the command, and runs nothing: no worktree, no lock, no harness.

Budget: before each phase, if the cost reported so far (across the units of one `ns watch`) has reached `limits.budget_usd`, the run ends with `budget`. On a subscription the reported cost is an estimate, so the cap is notional and unset by default; `max_units` and `--until` bound a night instead.

Output: final JSON `{unit, outcome: done|merged|stuck|budget|paused, phase, reason, pr, cost_usd, reset_at, artifact, phases:[...]}`. Exit 0 for done or merged, 1 for stuck, 2 for a usage or config error (bad definition, missing subscription login, harness not on PATH), 3 for budget, 4 for paused, 5 when another runner holds the lock.

## Merge

`merge.policy = "human"` (the default) ends a unit at `done` when `pr.md` passes. With `policy = "auto"`, `ns run` then runs its merge step. It is the only code in nightshift that merges, it only merges its own unit's PR, and only with squash:

1. `gh pr view <pr> --json state,headRefOid,mergeStateStatus`. `MERGED` already means something other than this step merged it: stuck. Not `OPEN`: stuck.
2. The PR head must equal the worktree HEAD, and `review.md` must be `pass` at that sha. Otherwise `done`, needing a human merge.
3. Branch protection requires PRs to be up to date with the base. `BEHIND`: `gh pr update-branch <pr>`. `DIRTY`, or a failed update: back to build with "rebase onto <default> and resolve conflicts" as `{feedback}`, which uses a build attempt.
4. `gh pr checks <pr> --watch`, killed after `ci_timeout_minutes` (stuck). Then `gh pr checks <pr> --json name,state,bucket,link`. No checks at all: `done`, needing a human merge. Any check not `pass` or `skipping`: back to build with the failing check names and the tail of `gh run view <id> --log-failed` as `{feedback}`, then verify, review and ship onto the same PR.
5. `gh pr diff <pr> --name-only`. Any path matching a `protected` glob (`**` crosses directories): `done` with reason "touches protected paths; needs a human merge". Protected paths cover the factory's own guardrails: CI config, the definition, the guard and merge code.
6. `gh pr merge <pr> --squash --delete-branch --match-head-commit <sha>`. If `gh pr view` then reports `MERGED`, the outcome is `merged`.

## Billing

`[defaults] billing = "subscription"` (the default) keeps claude runs on the Claude subscription login and off API credits:

- `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL` and `CLAUDE_CODE_USE_BEDROCK`/`_VERTEX`/`_FOUNDRY` are removed from every claude child process.
- `--bare`, which forces API-key auth, is dropped from the command.
- `ns run` refuses to start (exit 2) when `~/.claude/.credentials.json` is missing and `CLAUDE_CODE_OAUTH_TOKEN` is unset.

`billing = "api"` turns all three off. The user config's `[eval] billing` does the same for `ns eval`.

When the plan's usage limit is hit, `claude -p` ends with an error `result` event whose text mentions a usage limit, a rate limit, or "limit reached". That is not a phase failure: no attempt is used, the unit isn't stuck, and `ns run` ends with `paused` (exit 4), `reset_at` set when the message carries a reset time (`...|<unix seconds>`, or `resets 3am` in local time). Re-running resumes where it stopped.

## ns watch

```
ns watch [--once] [--until HH:MM] [--max-units N] [--dry-run] [--factory <dir>]
```

1. List open issues with `ready_label` via `gh issue list --json number,title,labels,body`.
2. Drop issues whose first-line `Blocked by: #a, #b` names any open issue, and issues that already have an open PR whose body says `Closes #n`.
3. Sort by the first matching `order` label, then by issue number.
4. Take the first. Swap `ready_label` for `in_progress_label`. `git fetch origin`, then `ns run --issue <n> --base origin/<default>` (in process, sharing the budget).
5. On `merged`, remove `in_progress_label`; GitHub closes the issue through the PR's `Closes #n`. On `done`, swap to `done_label`. On `stuck`, or a `done` that needs a human merge (protected paths, no CI), swap to `stuck_label` and comment the reason and the last artifact path. The comment carries the AI disclaimer. On `budget`, put `ready_label` back and stop.
6. On `paused`, keep `in_progress_label` and sleep until the reset time (30 minutes when unknown, then check again), then resume the same unit. If the reset is at or past `--until`, put `ready_label` back and stop cleanly.
7. Repeat until the queue is empty, `--until` passes (no new unit starts after it), `max_units` is reached, or the budget is spent.

An issue whose `Blocked by:` issue can't be read counts as blocked. `NS_NOW` (unix seconds) pins the clock for tests; sleeps then advance it instead of blocking. Output: `{units:[{issue, unit, outcome, reason, pr, cost_usd}], stopped, cost_usd}`.

`--once` takes one unit. `--dry-run` prints the ordered queue with skip reasons. `gh` runs with whatever `GH_TOKEN` the environment carries, so the account is chosen by whoever starts `ns watch`.

## Starting a night

```bash
export GH_TOKEN=$(gh auth token --user <account>)   # the account PRs should come from
ns watch --until 06:30 >> ~/.local/share/nightshift/watch.log 2>&1
```

Log in to claude with the subscription account first (`claude /login`). The phases run with permissions bypassed inside worktrees. Run on a machine where that's acceptable.
