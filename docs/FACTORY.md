# Factory definition and runner

The minimum slice of D15 to D17 (`docs/DESIGN.md`) that runs nightshift unattended: a definition in `.nightshift/`, `ns run` to drive one unit through the phases, and `ns watch` to pull units from the tracker overnight. Automations and scorers come later (#7).

## Layout

```
.nightshift/
  nightshift.toml          the definition root
  agents/<role>/agent.md   one role per phase (optional: built-in default prompts exist)
  runners/<name>.toml      exclusive locks a phase holds (optional; see "Runners")
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
gate = "scripts/ci-local.sh"       # build only. Unset: the ci-local row of docs/agents/stack.md. See "Gate"
[phases.verify]
skill = "ns-verify"
runner = "bench"                   # a runners/<name>.toml. Unset: no locks
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
split_label = "status:needs-define"   # set when triage split the issue into child issues
triage_label = "status:needs-triage"   # issues ns watch triages before it picks a unit
triage_per_night = 10              # cap on triage-only runs per `ns watch`; 0 turns the pass off
priority = ["priority:high", "priority:medium", "priority:low"]  # sorted on first; no match goes last
order = ["type:fix", "type:feat", "type:refactor", "type:test", "type:docs", "type:chore"]

[limits]
max_units = 8                      # optional cap per `ns watch` invocation; unset means no cap
budget_usd = 25.0                  # soft cap, summed from harness cost reports. Unset: no cap
                                   # under billing = "subscription", 25.0 under "api"

[merge]                            # absent table: policy = "human"
policy = "auto"                    # auto | human. See "Merge"
human_review = [".github/**", "scripts/check-private.sh", ".nightshift/**"]
ci_timeout_minutes = 30
ci_register_timeout = 3            # minutes to wait for the PR head's first check before "no CI"
```

Every table and key is optional; the defaults are the values above, except `limits.max_units` (unset: no cap), `limits.budget_usd` and `merge` as noted. A phase table may override `harness`, `model`, `timeout_minutes` and `max_attempts`, may set `runner`, and its `skill` defaults to `ns-<phase>`. Only `[phases.build]` may set `gate`, and not to an empty string. Unknown keys and unknown phases are errors. `ns factory validate` checks the file, every `runners/<name>.toml` and every `agents/<role>/agent.md` (role matches the directory, placeholders are known), and exits 1 on any problem.

## agents/<role>/agent.md

```markdown
---
role: build
---
Load the `{skill}` skill and run it for unit `{unit}` (issue {issue_url}).
Work in {worktree}. gates: {gates}.
```

Placeholders: `{skill}`, `{unit}`, `{issue}`, `{issue_url}`, `{worktree}`, `{gates}`, `{phase}`, `{attempt}`, `{feedback}` (the body of the artifact that sent work back, or the failing CI checks and log tail, empty on a first attempt). They are replaced in one pass, so placeholder-like text inside `{feedback}` stays literal. Without a file, `ns run` uses a built-in prompt of this shape, with a `Phase: <phase> (attempt <n>)` line and the feedback appended when there is any.

## Runners

A runner names the locks a phase must hold, so phases that share a Zephyr workspace or a hardware bench never overlap, across worktrees or across repos.

```toml
kind = "bench"                     # local (default) | bench
locks = ["bench-1", "zephyr-workspace"]
```

`kind` is a label for now: `bench` runs on this machine like `local`, and no runner is remote. A lock name is letters, digits, `.`, `_` and `-`, not starting with `.`, and no two lock names across runners may differ only by case, since a case-insensitive filesystem gives them one file. The runner's name is the file name, and a phase picks it with `runner = "<name>"`. `ns factory validate` reports a runner file that doesn't parse, an unknown key or `kind`, a bad lock name, two lock names that differ only by case, and a phase `runner` with no file. `ns run` refuses all of them with exit 2.

Before a phase with a runner starts, `ns run` takes every lock it names, sorted by name so two runs can't deadlock, and holds them until the phase's harness exits, whatever the outcome. A phase with no runner takes no locks.

Each lock is the file `<lock_dir>/<name>.lock`. While a run holds it, the file records that run's pid and unit; otherwise it is empty. `lock_dir` defaults to `<git-common-dir>/ns/locks`, which only other runs in the same repo see, and `ns-run.lock` already keeps those to one at a time. To share a bench or workspace between repos, point every repo's runs at one directory in the user config (`cli/README.md`, `[runners]`):

```toml
[runners]
lock_dir = "~/.local/state/nightshift/locks"
```

**A held lock makes the run wait**, not refuse. It prints `ns run: <unit> <phase> waits for lock <name> (held by pid <pid>, unit <unit>)` to stderr, logs a `lock_wait` event, and tries again with a backoff of up to 5 s until the holder lets go. The wait is bounded by the waiting phase's own timeout, counted from when the wait began, and by `ns watch --until`, whichever comes first; a hung holder, or a harness left running by a killed `ns`, can hold the lock for longer than its own run's timeout. On giving up, the run releases the locks it already took, prints `ns run: <unit> <phase> gave up waiting for lock <name> (held by pid <pid>, unit <unit>) at the phase timeout` (or `at --until`), and logs a `lock_wait_timeout` event with the lock, the holder and the `bound` (`timeout` or `until`). The phase timeout counts as a failed attempt, retried within `max_attempts`, and the out-of-attempts reason names the lock. `--until` ends the run as `budget` with that reason, and `ns watch` stops the night with `until`. A lock taken at or past `--until`, or taken with the budget spent, is released the same way (as `budget`), so the phase doesn't start after the deadline or on a spent budget. Exit 5 still means only the per-repo run lock (below). The lock is an OS `flock` on the file, so the kernel drops a run's hold when its process dies, and a file naming a dead pid is taken over at once. If `ns` is hard-killed (SIGKILL or a crash), the lock frees at once while its harness child may still be running, since the child does not inherit the lock.

## ns run

```
ns run <unit-id> [--issue <n>] [--from <phase>] [--gates stop|auto] [--dry-run] [--factory <dir>]
ns run --issue <n>                 # unit id <n>-<slug from the issue title>
ns run <unit-id> --base origin/main   # base for a new worktree (ns watch passes it)
```

Steps:

1. Create or reuse the worktree (`ns worktree new`, including setup).
2. Read `.ns/<unit>/*.md` frontmatter and pick the next phase from the state table.
3. Archive superseded artifacts. Before running phase P, move P's artifact, if present, to `.ns/<unit>/history/<artifact>-<n>.md`, with n one past the highest number already there for that artifact. Also archive every downstream artifact (order: brief, build, evidence, review, pr) unless it is `status: pass` and current, and every downstream artifact after one that was archived. Whatever remains in `.ns/<unit>/` is current, so the state table needs no timestamps.
4. Take the phase's runner locks (see "Runners"), waiting for any that another run holds. Run the phase's harness headless in write mode (`command_write`), with cwd set to the worktree and the rendered prompt on stdin. The phase starts in its own session and process group, so a `kill 0` inside it can't reach `ns`. Its environment carries `NS_UNIT`, `NS_PHASE`, `NS_ATTEMPT`, `NS_WORKTREE`, `NS_RUN_PID` (the `ns` process running the unit) and `NS_WATCH_PID` (the `ns watch` process, empty under a standalone `ns run`; under watch it equals `NS_RUN_PID`). Enforce the timeout by killing the process group. The built-in `claude` write command for `ns run` is `claude -p --permission-mode bypassPermissions --model {model} --output-format stream-json --verbose`: the phases need Bash, which `acceptEdits` can't grant headless. A configured claude command gets `--output-format stream-json --verbose` added if it lacks them. Stdout goes to `<git-common-dir>/ns/transcripts/<unit>/<phase>-<attempt>.jsonl`, and cost and tokens are parsed from it.
5. The attempt wrote its artifact if P's artifact exists after the run. If not, the attempt failed with "no artifact written" (or the timeout or exit code), and the files archived in step 3 move back, so the next decision sees the state as it was. A triage run that exits 0 without writing `brief.md` is the "triage decided a human or define is needed" row: stuck, no retry. A usage-limit stop also moves the files back (see "Billing").
   A phase killed at its timeout wrote nothing, whatever it left behind: its artifact moves to `.ns/<unit>/history/<artifact>-timeout-<n>.md` (n one past the highest timeout archive for that artifact), the attempt fails with "timed out after <n> min", and the run log's `phase` event names the archive as `archived`. When the phase then runs out of attempts, the stuck reason says the last attempt timed out and suggests raising its `timeout_minutes` or splitting the unit. `NS_PHASE_TIMEOUT_MS` overrides phase timeouts, for tests: `<ms>` for every phase, or `<phase>=<ms>[,<phase>=<ms>]` for the named ones.
6. Run the CI gate after a build that wrote `status: pass`, and after a review that moved HEAD (see "Gate"). A red gate sends the unit back to build.
7. Repeat until the unit is done, merged, split, stuck, paused, or out of budget.

An artifact is current when its frontmatter `sha` matches `git rev-parse --short HEAD` in the worktree by prefix (the lengths may differ), or when the unit's diff at that sha has the same `git patch-id` as at HEAD. The unit's diff is `git diff $(git merge-base <base> <sha>) <sha>`, with `<base>` the one the worktree was cut from (`origin/<base>` instead when that is ahead of the local branch): `--base`, else `origin/HEAD`'s branch (the local branch when present), else the branch checked out in the main worktree (not the literal `HEAD`, a deliberate change from the issue's text), else the detached sha. `git patch-id` ignores whitespace, so a whitespace-only change after review counts as current. A rebase that leaves the change alone keeps every artifact current. A sha no longer in the repo is not current. "`sha` ≠ HEAD" below means not current. Because of step 3, an artifact that exists is current. The table, as implemented, checked top to bottom:

| State | Next |
|---|---|
| `brief` split | **split**: child issues replaced the unit; `brief.md` moves to `history/`, so a parent sent back to the queue is triaged afresh |
| any artifact `blocked` | stuck |
| `pr`, `evidence` and `review` pass at HEAD | **done** (then the merge step under `merge.policy = auto`) |
| no `brief.md` | triage with an issue, else stuck "no brief" |
| `brief` not pass | stuck (triage decided a human or define is needed) |
| no `build`, or `build` fail | build (a fail's body is `{feedback}`) |
| no `evidence`, or evidence `sha` ≠ HEAD | verify |
| `evidence` fail | build, with its body as `{feedback}` |
| no `review`, or review `sha` ≠ HEAD | review |
| `review` fail (not an open Critical or Important in changed code after the last fix cycle; that is `blocked`) | build, with its body as `{feedback}` |
| no `pr`, or `pr` pass with `sha` ≠ HEAD | ship |
| `pr` fail | the phase its body names first (verify or review), with its body as `{feedback}`, else stuck |
| a phase out of attempts | stuck |

`skills/ns-auto` mirrors this table, the archiving and the currency rule for in-session runs; change them together. The skill is authoritative for in-session runs. Intended differences: its done condition comes from the contract, and a stuck unit gets no timeout archive.

A review that commits leaves `evidence.md` stale, so verify runs again before ship; `review.md` carries the new sha and stays valid. A CI failure in the merge step runs build again; build's commit leaves evidence, review and pr stale, so verify, review and ship follow onto the same PR. A ship that changes the unit's diff after review leaves `pr.md` at HEAD but `review.md` stale, so verify, review and ship run again; the per-phase `max_attempts` bounds the loop. The PR number comes from `pr.md`'s `pr:`, or from the newest archived `history/pr-<n>.md` that names one, so archiving never loses it.

Attempts are counted per invocation, so re-running `ns run` on a stuck unit gives each phase fresh attempts.

Hard rules, enforced in code whatever the prompts say:

- Phases never merge, never approve, and never push to the default branch. Only `ns run`'s own merge step merges (see "Merge").
- Before the first phase, `ns run` records `git ls-remote origin` for the default branch (skipped without an `origin` remote). After every phase it reads it again. If it moved to a commit reachable from the unit's HEAD, this run pushed to it: stuck with "default branch moved". A move to anything else is another actor (a human merge, another machine): it is logged as `default_moved_by_other_actor` and becomes the new baseline.
- After every phase, if the unit has a known PR (`pr:` in `pr.md` or its newest archived copy, a number or a URL ending in one), `gh pr view <n> --json state`. `MERGED` means a phase merged it: stuck with "PR merged by run".
- Phases don't write the operator's personal memory. Every claude phase and `ns eval` trial runs with `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, set over whatever the operator's environment says, so nothing lands in `~/.claude/projects/`. Other harnesses don't get the variable. A lesson worth keeping goes through `ns-improve` into a skill, where review sees it. `ns doctor` reports this as `claude_phases.auto_memory: false`.
- One unit at a time per repo. A lock file in the common git dir (`ns-run.lock`, holding pid and unit) refuses a second runner with exit 5. A lock whose pid is dead is removed. Runner locks are separate and wait instead (see "Runners").

Run log: append one JSON line per event to `.git/ns/runs.jsonl` in the common git dir: unit, phase, attempt, decision, artifact status, sha, cost, tokens, wall time, exit. Start, end, breaches, CI failures, gate runs and merges are events too. `ns run --dry-run` prints the next decision, the rendered prompt and the command, and runs nothing: no worktree, no lock, no harness.

Budget: before each phase, if the cost reported so far (across the units of one `ns watch`) has reached `limits.budget_usd`, the run ends with `budget`. On a subscription the reported cost is an estimate, so the cap is notional and unset by default; `--until`, an optional `max_units`, and usage-limit pauses bound a night instead.

Output: final JSON `{unit, outcome: done|merged|split|stuck|budget|paused, phase, reason, pr, cost_usd, reset_at, artifact, phases:[...]}`. A `split` names the triage phase, the archived `brief.md` as the artifact, and the reason `brief.md is split: <its first line>`. Exit 0 for done, merged or split, 1 for stuck, 2 for a usage or config error (bad definition, missing subscription login, harness not on PATH), 3 for budget, 4 for paused, 5 when another runner holds the lock.

### Gate

Build's artifact is the agent's own word that checks pass. The gate is a deterministic check `ns run` runs itself, so a build that fails CI goes back to build before verify, review and the PR.

The command is `[phases.build] gate`. Unset, it is the `ci-local` row of the Commands table in `docs/agents/stack.md` in the main checkout: a table row whose first cell is `ci-local` (backticks optional), with the command as the backticked span that opens the second cell (it may contain `|`):

```markdown
| ci-local | `scripts/ci-local.sh` | 2 min |
```

With neither, no gate runs. Both are read from the main checkout when `ns run` starts, so a phase can't change the command. The script it runs lives in the worktree, though, so the gate is not tamper-proof.

`ns run` runs the command with `sh -c` in the worktree after a build that wrote `status: pass`, and again after a review that moved HEAD (a fix cycle that committed). It doesn't run again at a HEAD where it already went green in this invocation. It is killed at the build phase's `timeout_minutes` (`NS_GATE_TIMEOUT_MS` overrides it, for tests). Only the last 16 KiB of its log are read for the tail. Output goes to `<git-common-dir>/ns/transcripts/<unit>/gate-<phase>-<attempt>.log`.

Exit 0 is green and the run goes on as the state table says. A non-zero exit, a signal or the timeout is red: the next phase is build, with the command, how it failed and the last 40 lines of its output (at most 3000 bytes) as `{feedback}`. That build uses an attempt against `max_attempts`, so a gate that stays red ends with "build is out of attempts". Each run is a `gate` event in the run log with `phase` (what triggered it), `attempt`, `command`, `sha`, `exit`, `timed_out`, `wall_s`, `green` and `log`. `ns run --dry-run` shows the command as `gate`.

## Merge

`merge.policy = "human"` (the default) ends a unit at `done` when `pr.md` passes. With `policy = "auto"`, `ns run` then runs its merge step. It is the only code in nightshift that merges, it only merges its own unit's PR, and only with squash:

1. `gh pr view <pr> --json state,headRefOid,mergeStateStatus`. `MERGED` already means something other than this step merged it: stuck. Not `OPEN`: stuck.
2. The PR head and `review.md` must be current (see the state table), and `review.md` must be `pass`. Otherwise `done`, needing a human merge. The merge in step 7 matches the PR head, including the head from before a `BEHIND` update when `update-branch` reports none; this goes beyond the issue's literal text on purpose, so a rebase after review still merges the reviewed head.
3. Branch protection requires PRs to be up to date with the base. `BEHIND`: `gh pr update-branch <pr>`. `DIRTY`, or a failed update: back to build with "rebase onto <default> and resolve conflicts" as `{feedback}`, which uses a build attempt.
4. GitHub may not have registered checks on a head it just received, so the step first polls `gh api repos/{owner}/{repo}/commits/<sha>/check-runs` and `.../status` for the PR head (the one `update-branch` returned, if it ran) until either reports a check. Polls back off from 5 s to 30 s, bounded by `ci_register_timeout` minutes. None by then: `done`, needing a human merge, with reason "no CI checks registered within <n> min on PR #<pr>; needs a human merge". If the last poll's `gh api` call failed (auth, rate limit, 404) or printed something other than a count, the reason is instead "could not query CI checks on PR #<pr>: <error>; needs a human merge". Then `gh pr checks <pr> --watch`, killed after `ci_timeout_minutes` (stuck). Then `gh pr checks <pr> --json name,state,bucket,link`. No checks at all: `done`, needing a human merge. Any check not `pass` or `skipping`: back to build with the failing check names and the tail of `gh run view <id> --log-failed` as `{feedback}`, then verify, review and ship onto the same PR.
5. `gh pr diff <pr> --name-only`. Any path matching a `human_review` glob (`**` crosses directories): `done` with reason "changes files that need human review; needs a human merge". Files that need human review cover the factory's own guardrails: CI config, the definition, the guard and merge code.
6. Marked regions (below): HEAD is diffed against its merge base with `origin/<default>`. A changed line inside a region, in the base or the head version, or an added, removed or moved marker line: `done` with reason "changes code in a human-review region; needs a human merge (<file>:<start>-<end>: <reason>)". If the merge base can't be found, the reason says so and the unit still needs a human merge.
7. `gh pr merge <pr> --squash --delete-branch --match-head-commit <sha>`. If `gh pr view` then reports `MERGED`, the outcome is `merged`.

### Marked regions

`human_review` guards whole files. To guard part of a file, fence it. A region runs from a line containing `ns:human-review start`, optionally followed by `: <reason>`, through the next line containing `ns:human-review end`, marker lines included. Detection is by substring, so any comment syntax works. `start` and `end` may be followed by anything except a letter, digit, `_`, `-`, a quote or a backtick (so `start*/` and `end-->` count, but `endpoint` and a marker quoted in backticks do not). A firmware project fencing its brake limits, with the keyword written as `[ns:human-review]` so this page holds no live marker; drop the brackets in real code:

```c
int brake_ramp(int now) { return now * 2; }

/* [ns:human-review] start: brake torque limits, signed off by the safety lead */
#define MAX_TORQUE_NM 420
#define TORQUE_RAMP_MS 150
/* [ns:human-review] end */
```

A PR that changes `brake_ramp` merges on its own. One that changes `MAX_TORQUE_NM`, or moves or deletes either marker, ends at `done` with "changes code in a human-review region; needs a human merge (src/brake.c:3-6: brake torque limits, signed off by the safety lead)".

`ns check-markers [path]` checks every tracked file and exits 1 on a start with no end, an end with no start, or a start nested inside a region, naming file and line. Run it in CI next to the tests. `ns-no-comments` keeps marker lines. nightshift marks no regions in its own code; this example is the only one in the repo.

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

1. Run the triage pass (see "Triage pass"), so issues filed tonight can join tonight's queue.
2. List open issues with `ready_label` via `gh api --paginate repos/{owner}/{repo}/issues`. The REST list carries each author's association, which `gh issue list --json` lacks.
3. Drop issues whose author is outside the team (see Trust), issues whose first-line `Blocked by: #a, #b` names any open issue, and issues that already have an open PR whose body says `Closes #n`.
4. Sort by the first matching `priority` label, then the first matching `order` label, then issue number. An issue with no `priority` label sorts after the last one.
5. Take the first. Swap `ready_label` for `in_progress_label`. `git fetch origin`, then `ns run --issue <n> --base origin/<default>` (in process, sharing the budget).
6. On `merged`, remove `in_progress_label`; GitHub closes the issue through the PR's `Closes #n`. On `done`, swap to `done_label`. On `split`, swap to `split_label` and post nothing: triage's notes already list the children, and the parent waits on them, neither queued nor triaged again, until a human closes it. On `stuck`, or a `done` that needs a human merge (files that need human review, no CI), swap to `stuck_label` and comment the reason and the last artifact path. When the last artifact is a `review.md`, the comment also lists its open Critical and Important findings, each with its title and location. The comment carries the AI disclaimer. On `budget`, put `ready_label` back and stop.
7. On `paused`, keep `in_progress_label` and sleep until the reset time (30 minutes when unknown, then check again), then resume the same unit. If the reset is at or past `--until`, put `ready_label` back and stop cleanly.
8. Repeat until the queue is empty, `--until` passes (no new unit starts after it), `max_units` is reached, or the budget is spent.

`--until HH:MM` is local time, from `TZ` or the system zone, so `06:30` means 06:30 where `ns watch` runs, across DST changes. Every time `ns watch` prints for a person (`until`, `reset_at`, the "sleeping until" line) is local, in RFC 3339 form with its offset: `2026-10-09T06:30:00-04:00`. The `ts` of each event in `runs.jsonl` stays UTC (`...Z`).

### Triage pass

Review files escapes and follow-ups as `needs-triage` issues, and only `ready_label` issues are queued, so before each unit `ns watch` triages what nobody has yet:

1. List every open issue (`gh api --paginate repos/{owner}/{repo}/issues?state=open`). A candidate carries `triage_label`, or no state label at all. A state label is any `status:` label or one of the `[queue]` labels. Issues whose author is outside the team, and issues an open or merged PR closes, are skipped, as in the queue. A blocked issue is still a candidate: triage can triage it, and the queue skips it until its blocker closes.
2. Drop issues already given a triage-only run tonight, whatever came of it, and issues whose unit ended tonight. Sort the rest like the queue.
3. Run the first as a triage-only run: after the fetch and skills sync a unit gets, in process, `ns run --issue <n> --base origin/<default>` with the triage phase forced, even over an earlier `brief.md`, run once, and then stopped. It never sets `in_progress_label`, so triage applies the state and priority itself (ready-for-agent, needs-info, needs-define, wontfix, ...). The prompt ends with a `Triage only:` line telling the phase to stop after the outcome and hand off to no other phase. A ready-for-agent outcome writes `brief.md` in the unit's worktree, so the unit later starts at build.
4. Repeat from 1 until no candidate is left or `triage_per_night` runs have started, then pick the unit. The list is read again before every unit, so issues the night's units file are triaged before the next pick, and an issue triaged ready sorts into the queue by its priority.

A triage-only run is `done` when the phase exits 0, and `stuck` when it exits non-zero, is killed, or times out. A run that can't start (the issue can't be read, the unit's worktree path is taken) is recorded as `error` and the pass goes on to the next candidate. When a run fails to start with the same error as the last one that did, the cause is global (a missing login, an unwritable lock dir), so the pass is off for the rest of the night and the unit's own run reports the error. Either way the issue keeps the labels triage left, gets no comment from `ns watch`, and is not tried again that night. A run whose every attempt failed instantly counts toward the same harness breaker as units: two such runs in a row, triage-only or unit, stop the night with `harness failing`. A usage limit sleeps until the reset like a unit, then tries the candidates again without spending the cap; a reset at or past `--until` stops the night with no label change. `--until` and the budget are checked before every triage-only run. Triage-only runs are not units: `--once` and `max_units` count units only, so `--once` triages, then builds one unit.

Under `gates = "stop"` triage only recommends and applies nothing, so the pass is off (`--dry-run` shows `triage_per_night` as 0 and no candidates). Every triage-only run creates the unit's worktree and branch; one that doesn't end ready-for-agent leaves them for the next attempt, like a stuck unit. Only `status:` labels and the `[queue]` labels count as states, so in a tracker whose other states lack the `status:` prefix (a bare `needs-info`), those issues look unlabelled and are triaged again each night.

Before each unit, after the fetch, `ns watch` keeps installed skills current. A unit's worktree already has the skills checked into the repo at `origin/<default>`, but a skill installed as a symlink into the main checkout (`~/.agents/skills/*`, `~/.claude/skills/*`) would otherwise run whatever the checkout held when the night began. So when any installed symlink resolves to a skill dir (one with a `SKILL.md`) in the main checkout, and that checkout is on the default branch with no changes to tracked files, `ns watch` fast-forwards it to `origin/<default>` with the checkout's git hooks off. It then reruns `ns install` for each dir of `ns-*` skills it found, so a skill added tonight is linked and a removed one is pruned. It logs all this to stderr and as a `skills_synced` event (`from`, `to`, `skills`, `relinked`) in the run log. On another branch, a detached HEAD, local changes or diverged history, it changes nothing: it prints a warning naming the reason and the skills, logs a `skills_stale` event, and runs the unit anyway. The same reason warns once, not before every unit. It never resets, stashes or switches branches. Skills installed from anywhere else, such as a consumer's nightshift checkout, are never touched.

An issue whose `Blocked by:` issue can't be read counts as blocked. `NS_NOW` (unix seconds) pins the clock for tests; sleeps then advance it instead of blocking. Output: `{units:[{issue, unit, outcome, reason, pr, cost_usd}], triaged:[{issue, unit, outcome, reason, cost_usd, state}], stopped, until, cost_usd, started_with}`; `until` is null without `--until`. A triaged record's `state` lists the issue's state labels after the run (null when they couldn't be read); a paused one has `reset_at` instead, and an `error` one only `issue`, `outcome` and `reason`.

`ns watch` reads the user config and the factory definition once, at start, and every unit uses that copy. Editing or breaking either file mid-night changes nothing until the next `ns watch`. The build gate command is read at start too, so a `docs/agents/stack.md` that a fast-forward of the main checkout brings in (above) takes effect at the next `ns watch`. It logs each file's path and sha256 to stderr, and `started_with` carries the same `{config, factory}` pair of `{path, sha256}` (`sha256` is null for a missing file).

`--once` takes one unit. `--dry-run` prints the ordered queue, with each issue's `priority_label` and `order_label`, and the skip reasons, then the ordered triage candidates (`triage`, each with why it needs triage), their skip reasons (`triage_skipped`) and `triage_per_night`. `gh` and the phases get `GH_TOKEN` from `[forge.github]` in the user config (`cli/README.md`, `[forge]`). A `GH_TOKEN` already set in the environment wins, so whoever starts `ns watch` can still pick the account.

## Quality

```
ns quality [--since YYYY-MM-DDTHH:MM:SSZ|YYYY-MM-DD] [--json]
```

`ns quality` measures whether build writes code that passes review. It reads every unit's review artifacts in the linked worktrees (`.ns/<unit>/review.md`, `review/cycle-*.md`, and archived `history/review*`), plus the run log, and prints JSON. The main checkout is skipped. First-pass numbers come from a unit's oldest review artifact, since a rebuilt unit's later review starts from a different diff. Everything else comes from the newest.

| Metric | What it counts | Target |
|---|---|---|
| First-pass yield | Units whose first review pass raised no Critical or Important finding in changed code, over units where that is known | Rising |
| Findings per 100 lines | Critical and Important findings in changed code from the first pass, per 100 changed lines, overall and per axis. A finding with two axes counts once in each | Falling |
| Cycles to clean | Fix cycles a unit needed to end `pass` with no open Critical or Important in changed code (median, max). Units that never got there are `not_clean` | Falling |
| Leftovers | Critical and Important findings in changed code still `open` after the third fix cycle | 0 |
| Escapes | Findings with `Scope: pre-existing` (D29): defects in code the unit didn't change, so an earlier unit's review let them through. Each is blamed (`git blame` at the reviewed commit, the frontmatter's `sha:`, else `head:`, else `base:`) to the commit that introduced the line, and to the PR in its subject (`(#123)`) | Falling |

An escape counts only as an escape: it never makes a unit's first pass dirty, never counts toward findings per 100 lines, and never blocks. A finding dismissed in review counts toward none of these: review judged it wrong. `trend` repeats the numbers for each local day (the newest artifact's `updated:`), and `--since` keeps only units whose newest artifact was updated, and run-log events, at or after a UTC instant. A bare date means 00:00:00 UTC, whatever `TZ` is, and an undated unit or event is dropped. `per_unit` has each unit's numbers, with `changed_lines`, which keeps the review cost of a large unit visible now that size never blocks one (D37), `reached_clean` (`clean`, `not_clean` or `unknown`) and its review runs, review cost and outcome from the run log. `run_log.units_without_artifacts` names units the log shows reviewed whose worktree is gone.

The numbers need the finding fields `ns-review` writes: `Axis`, `Scope`, `Cycle` and `Status`. Older artifacts lack some of them, so `ns quality` falls back:

- No `Axis`: the axes named in `Raised by`.
- No `Cycle`: a finding is first-pass when the attempt ran 0 fix cycles, or when it was `fixed (cycle 1, ...)`. Otherwise the first pass's cycle file (`cycle-0.md` or `cycle-1.md`) is its record, counted from the ids it lists and the axes on their lines; its ids are not matched against `review.md`, because old artifacts reuse ids across passes. With neither, a unit with fix cycles still counts as dirty, because fix cycles only run for an open Critical or Important, but it is left out of the per-100-lines numbers.
- No `Scope`: not an escape.
- `cycles:` of 1 or more with no Critical or Important `fixed`: cycles to clean is `unknown`, because older artifacts counted review passes there.
- No `Change size:` in the Summary: `git diff --shortstat <base>...<head> -- . ':!.ns'` from the frontmatter shas.
- A heading such as `### I1-I7 (cycle 1). ...` is seven findings sharing its fields. One-line `- S1. ... (axis). Open.` items inside a severity section are findings too.
- An id prefix other than `C`, `I` or `S`, such as `### E1.` under an `## Escapes` section or `### D1.` under `## Dismissed`, is still a finding. Its severity comes from a `raised as Important` note in its fields, else it is unknown: counted in `findings.unknown_severity`, never blocking. Its `Scope: pre-existing` still makes it an escape, whatever its section.
- A first attempt that ended without `pass` and lists no findings (a reviewer slice couldn't run, a timeout) has an unknown first pass, not a clean one.

These are heuristics. A finding's severity comes from its `C`, `I` or `S` id prefix, so an `I5` noted as downgraded to a Suggestion still counts as Important, and a first-pass cycle file's ids all count, including any it lists as dismissed. An archived attempt's cycle files are read from `history/<stem>/` when present; `ns run` archives only `review.md`, so usually only the newest attempt has them.

`gaps` reports what was missing: findings without a severity or each field, headings with a `Status:` that didn't parse as findings (`### C3-1.`), units whose first pass, first-pass count or change size is unknown, and artifacts that couldn't be read (no frontmatter, or cycle files with no review artifact). Read the numbers alongside them.

## Trust

Phases run unattended with permissions bypassed, and on a public repo anyone can open an issue or comment on one. The team is the authors whose association is `OWNER`, `MEMBER` or `COLLABORATOR`.

- **Only team-authored issues are queued.** `ns watch` skips any other issue with reason `author outside the team`. A missing association counts as outside the team.
- **Only team comments are instructions.** Phases quote other text as data and never act on it. The rule lives in `skills/ns-contract/SKILL.md` under "Untrusted issue content", and the skills that read issues point there.

What remains: the team's own text is trusted in full, so a team member who pastes untrusted text into an issue or comment passes it through as instructions. `ns run --issue <n>` started by hand runs any issue, whoever wrote it, and only the skills' rule guards its comments.

## Starting a night

```bash
ns watch --until 06:30 >> ~/.local/share/nightshift/watch.log 2>&1
```

Put the account PRs should come from in the user config first, and check it with `ns doctor`:

```toml
[forge.github]
token_command = "gh auth token --user <account>"
```

Log in to claude with the subscription account first (`claude /login`). The phases run with permissions bypassed inside worktrees. Run on a machine where that's acceptable.
