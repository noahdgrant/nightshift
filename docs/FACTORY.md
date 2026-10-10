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
priority = ["priority:high", "priority:medium", "priority:low"]  # sorted on first; no match goes last
order = ["type:fix", "type:feat", "type:refactor", "type:test", "type:docs", "type:chore"]

[limits]
max_units = 8                      # optional cap per `ns watch` invocation; unset means no cap
budget_usd = 25.0                  # soft cap, summed from harness cost reports. Unset: no cap
                                   # under billing = "subscription", 25.0 under "api"
memory_mb = 16384                  # optional cap on each phase and gate, in MiB. Unset: no cap.
                                   # See "Memory cap"
parallel = 2                       # units ns watch runs at once; --parallel wins. Unset: 1.
                                   # See "Parallel units"

[merge]                            # absent table: policy = "human"
policy = "auto"                    # auto | human. See "Merge"
human_review = [".github/**", "scripts/check-private.sh", ".nightshift/**"]
ci_timeout_minutes = 30
ci_register_timeout = 3            # minutes to wait for the PR head's first check before "no CI"
```

Every table and key is optional; the defaults are the values above, except `limits.max_units` and `limits.memory_mb` (unset: no cap), `limits.parallel` (unset: 1), `limits.budget_usd` and `merge` as noted. A phase table may override `harness`, `model`, `timeout_minutes` and `max_attempts`, may set `runner`, and its `skill` defaults to `ns-<phase>`. Only `[phases.build]` may set `gate`, and not to an empty string. Unknown keys and unknown phases are errors. One removed key is not: `[queue] triage_per_night` still loads, does nothing, and gets a warning, on stderr from `ns run` and `ns watch` and in the `warnings` list of `ns factory validate`, which still passes. A file that set it to 0 to turn the triage pass off now runs the pass, and the warning says so: only `gates = "stop"` turns it off. `ns factory validate` checks the file, every `runners/<name>.toml` and every `agents/<role>/agent.md` (role matches the directory, placeholders are known), and exits 1 on any problem.

`[worktree] setup` commands run with `sh -c` in a unit's worktree, in order, stopping at the first that fails, with `NS_UNIT`, `NS_WORKTREE` and `NS_MAIN_ROOT` set. `ns worktree new` (and so `ns run`) runs them on a new worktree, and again on an existing one whose setup never finished: a stop, a crash or a failed command leaves no record that it did. When every command passes, it records the unit's id in the worktree's own git dir (`ns-setup-done` under `git rev-parse --absolute-git-dir`), outside the worktree's files and gone when the worktree is removed. `ns worktree new --no-setup` skips them and records nothing, so the next `ns worktree new` without the flag runs them. `ns worktree setup <unit>` reruns them whatever the record says. Until setup passes, every `ns worktree new` for the unit runs it again and exits 1 on a failure. So each command must be safe to rerun, on a worktree it already set up in full or in part: `git submodule update --init` is.

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

Each lock is the file `<lock_dir>/<name>.lock`. While a run holds it, the file records that run's pid and unit; otherwise it is empty. `lock_dir` defaults to `<git-common-dir>/ns/locks`, which only other runs in the same repo see. To share a bench or workspace between repos, point every repo's runs at one directory in the user config (`cli/README.md`, `[runners]`):

```toml
[runners]
lock_dir = "~/.local/state/nightshift/locks"
```

**A held lock makes the run wait**, not refuse. It prints `ns run: <unit> <phase> waits for lock <name> (held by pid <pid>, unit <unit>)` to stderr, logs a `lock_wait` event, and tries again with a backoff of up to 5 s until the holder lets go. The wait is bounded by the waiting phase's own timeout, counted from when the wait began, and by `ns watch --until`, whichever comes first; a hung holder, or a harness left running by a killed `ns`, can hold the lock for longer than its own run's timeout. On giving up, the run releases the locks it already took, prints `ns run: <unit> <phase> gave up waiting for lock <name> (held by pid <pid>, unit <unit>) at the phase timeout` (or `at --until`), and logs a `lock_wait_timeout` event with the lock, the holder and the `bound` (`timeout` or `until`). The phase timeout counts as a failed attempt, retried within `max_attempts`, and the out-of-attempts reason names the lock. `--until` ends the run as `budget` with that reason, and `ns watch` stops the night with `until`. A lock taken at or past `--until`, or taken with the budget spent, is released the same way (as `budget`), so the phase doesn't start after the deadline or on a spent budget. Exit 5 still means only the unit's run lock (below) or, for `ns watch`, its watch lock. The lock is an OS `flock` on the file, so the kernel drops a run's hold when its process dies, and a file naming a dead pid is taken over at once. If `ns` is hard-killed (SIGKILL or a crash), the lock frees at once while its harness child may still be running, since the child does not inherit the lock.

## ns run

```
ns run <unit-id> [--issue <n>] [--from <phase>] [--gates stop|auto] [--dry-run] [--factory <dir>]
ns run --issue <n>                 # unit id <n>-<slug from the issue title>
ns run <unit-id> --base origin/main   # base for a new worktree (ns watch passes it)
```

Steps:

1. Create or reuse the worktree (`ns worktree new`, including setup, which runs again on a worktree whose setup never finished). Worktrees are created one at a time per repo: `ns worktree new` holds the worktree lock, an OS `flock` on `ns-worktree.lock` in the common git dir, from before it lists the worktrees until the new one and its `.ns/<unit>/` exist, and waits while another holds it. `git worktree add` from a remote base writes the branch's upstream to `.git/config`, and two at once fail to lock it. Setup runs after the lock is let go.
2. Read `.ns/<unit>/*.md` frontmatter and pick the next phase from the state table.
3. Archive superseded artifacts. Before running phase P, move P's artifact, if present, to `.ns/<unit>/history/<artifact>-<n>.md`, with n one past the highest number already there for that artifact. Also archive every downstream artifact (order: brief, build, evidence, review, pr) unless it is `status: pass` and current, and every downstream artifact after one that was archived. Whatever remains in `.ns/<unit>/` is current, so the state table needs no timestamps.
4. Take the phase's runner locks (see "Runners"), waiting for any that another run holds. Run the phase's harness headless in write mode (`command_write`), with cwd set to the worktree and the rendered prompt on stdin. The phase starts in its own session and process group, so a `kill 0` inside it can't reach `ns`. Its environment carries `NS_UNIT`, `NS_PHASE`, `NS_ATTEMPT`, `NS_WORKTREE`, `NS_RUN_PID` (the `ns run` process running the unit) and `NS_WATCH_PID` (the `ns watch` process, empty under a standalone `ns run`; under watch, the parent of `NS_RUN_PID`, since each unit runs as its own `ns run`; a triage-only run, which `ns watch` runs itself, has both set to `ns watch`). Enforce the timeout by killing the process group. The built-in `claude` write command for `ns run` is `claude -p --permission-mode bypassPermissions --model {model} --output-format stream-json --verbose`: the phases need Bash, which `acceptEdits` can't grant headless. A configured claude command gets `--output-format stream-json --verbose` added if it lacks them. Stdout goes to `<git-common-dir>/ns/transcripts/<unit>/<phase>-<attempt>.jsonl`, and cost and tokens are parsed from it.
5. The attempt wrote its artifact if P's artifact exists after the run. If not, the attempt failed with "no artifact written" (or the timeout or exit code), and the files archived in step 3 move back, so the next decision sees the state as it was. A triage run that exits 0 without writing `brief.md` is the "triage decided a human or define is needed" row: stuck, no retry. A usage-limit stop also moves the files back (see "Billing").
   A phase killed at its timeout wrote nothing, whatever it left behind: its artifact moves to `.ns/<unit>/history/<artifact>-timeout-<n>.md` (n one past the highest timeout archive for that artifact), the attempt fails with "timed out after <n> min", and the run log's `phase` event names the archive as `archived`. When the phase then runs out of attempts, the stuck reason says the last attempt timed out and suggests raising its `timeout_minutes` or splitting the unit. `NS_PHASE_TIMEOUT_MS` overrides phase timeouts, for tests: `<ms>` for every phase, or `<phase>=<ms>[,<phase>=<ms>]` for the named ones.
   A phase that hit the memory cap (see "Memory cap") or was killed by a signal `ns` didn't send is cut short the same way: its artifact moves to `history/<artifact>-killed-<n>.md`, the attempt fails with "exceeded the <n> MB memory limit" or "was killed by SIGKILL" (the signal's name), and when the phase runs out of attempts the stuck reason ends with that, as in "build is out of attempts (2): the last attempt exceeded the 16384 MB memory limit".
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
- One run at a time per unit. The run lock, an OS `flock` on `ns-run-<unit>.lock` in the common git dir, refuses a second `ns run` of the same unit with exit 5; runs of other units in the repo go ahead. While held, the file records the holder's pid, unit and issue; otherwise it is empty. The kernel drops a dead run's hold, so a file left by a crash is taken over at once. Runner locks, the merge lock (see "Merge") and the worktree lock (step 1) are separate and wait instead (see "Runners").

Run log: append one JSON line per event to `.git/ns/runs.jsonl` in the common git dir: unit, phase, attempt, decision, artifact status, sha, cost, tokens, wall time, exit. Start, end, breaches, CI failures, gate runs and merges are events too. `ns run --dry-run` prints the next decision, the rendered prompt and the command, and runs nothing: no worktree, no lock, no harness.

Quality records: when a unit ends `merged`, `done` or `stuck`, `ns run` writes one record per review attempt to the `nightshift/quality` branch on `origin` and logs a `quality_record` event (see "Quality"). A unit with no review artifact adds no record. A push that fails, or a repo with no `origin`, leaves the records in the outbox and never changes the unit's outcome.

Cleanup: when the merge step merged the unit's PR and its records are saved (pushed, or in the outbox), `ns run` removes the unit's worktree and its local `ns/<unit>` branch and logs a `cleanup` event. A worktree with uncommitted changes to tracked files, or a branch holding changes the merged PR head doesn't, stays. See "Cleanup".

Budget: before each phase, if the cost reported so far (across the units of one `ns watch`) has reached `limits.budget_usd`, the run ends with `budget`. On a subscription the reported cost is an estimate, so the cap is notional and unset by default; `--until`, an optional `max_units`, and usage-limit pauses bound a night instead.

Output: final JSON `{unit, outcome: done|merged|split|stuck|budget|paused, phase, reason, pr, cost_usd, reset_at, artifact, worktree, cleanup, phases:[...]}`. `cleanup` is null unless the unit merged; then it is `{unit, path, branch, issue, pr, removed}`, with a `reason` when the worktree stays. A `split` names the triage phase, the archived `brief.md` as the artifact, and the reason `brief.md is split: <its first line>`. Exit 0 for done, merged or split, 1 for stuck, 2 for a usage or config error (bad definition, missing subscription login, harness not on PATH), 3 for budget, 4 for paused, 5 when another `ns run` holds the unit's run lock.

### Gate

Build's artifact is the agent's own word that checks pass. The gate is a deterministic check `ns run` runs itself, so a build that fails CI goes back to build before verify, review and the PR.

The command is `[phases.build] gate`. Unset, it is the `ci-local` row of the Commands table in `docs/agents/stack.md` in the main checkout: a table row whose first cell is `ci-local` (backticks optional), with the command as the backticked span that opens the second cell (it may contain `|`):

```markdown
| ci-local | `scripts/ci-local.sh` | 2 min |
```

With neither, no gate runs. Both are read from the main checkout when `ns run` starts, so a phase can't change the command. The script it runs lives in the worktree, though, so the gate is not tamper-proof.

`ns run` runs the command with `sh -c` in the worktree after a build that wrote `status: pass`, and again after a review that moved HEAD (a fix cycle that committed). It doesn't run again at a HEAD where it already went green in this invocation. It is killed at the build phase's `timeout_minutes` (`NS_GATE_TIMEOUT_MS` overrides it, for tests). Only the last 16 KiB of its log are read for the tail. Output goes to `<git-common-dir>/ns/transcripts/<unit>/gate-<phase>-<attempt>.log`.

Exit 0 is green and the run goes on as the state table says. A non-zero exit, a signal, the memory cap or the timeout is red: the next phase is build, with the command, how it failed and the last 40 lines of its output (at most 3000 bytes) as `{feedback}`. How it failed names the signal ("was killed by SIGKILL") or the cap ("exceeded the 16384 MB memory limit") when one ended it, and so does the stuck reason: "build is out of attempts (2): the CI gate exceeded the 16384 MB memory limit". That build uses an attempt against `max_attempts`, so a gate that stays red ends with "build is out of attempts". Each run is a `gate` event in the run log with `phase` (what triggered it), `attempt`, `command`, `sha`, `exit`, `signal`, `timed_out`, `memory_exceeded`, `wall_s`, `green` and `log`. `ns run --dry-run` shows the command as `gate`.

### Memory cap

One runaway test binary can take all of a machine's memory and, with it, the night. `[limits] memory_mb` caps each phase and each gate run at that many MiB. It is off by default. Under the cgroup the cap covers the run with everything it starts; under the RLIMIT_AS fallback it applies to each process on its own (see below).

When it is set, `ns run` and `ns watch` probe once at start (a dry run doesn't) and print which mechanism they use; each unit's `start` event in the run log records it as `memory_cap: {mb, via, fallback}`. With a cap set, `ns run` checks that the harness binary exists before wrapping it, so a missing one is still the usage error, exit 2.

- **cgroup** (`via: "systemd-run"`), where `systemd-run --user` accepts the scope's properties and sets its `memory.max` (the probe starts a scope with the same properties as a real run): the command runs as `systemd-run --user --scope -p MemoryMax=<n>M -p MemorySwapMax=0 -p OOMPolicy=stop`. At the cap the kernel kills a process in the scope and systemd stops the rest. `ns` reads the scope's result from systemd (`oom-kill`), so the cap is detected exactly.
- **RLIMIT_AS** (`via: "prlimit"`), otherwise: the command runs as `prlimit --as=<bytes>`, and `fallback` says why the cgroup wasn't used. Nothing is killed at the cap. An allocation past it fails and the program decides what to do. `ns` counts a run as over the cap when it failed and its output (the gate log's tail, or the harness's stderr) holds a line with a common allocation-failure message (Rust's `memory allocation of <n> bytes failed`, a line starting `MemoryError`, `std::bad_alloc`, `Cannot allocate memory`, Go's `fatal error: runtime: out of memory`, V8's `Fatal process out of memory`). That is a heuristic: a failure that prints none of them reads as an ordinary failure. The limit is per process, not per run: each process the run starts gets its own `memory_mb`, so `cargo test` or `make -j8` can use many times the cap in total. RLIMIT_AS also counts reserved address space, not memory in use, so runtimes that reserve large ranges (sanitizers, the JVM, Go, V8) need a cap well above what they use.
- Neither works: `ns run` and `ns watch` exit 2 before any phase runs.

A phase or gate over the cap fails that attempt with "exceeded the <n> MB memory limit" (see step 5 and "Gate"), the unit carries on or goes stuck through the normal attempt rules, and `ns watch` goes on to the next unit.

## Merge

`merge.policy = "human"` (the default) ends a unit at `done` when `pr.md` passes. With `policy = "auto"`, `ns run` then runs its merge step. It is the only code in nightshift that merges, it only merges its own unit's PR, and only with squash:

1. `gh pr view <pr> --json state,headRefOid`. `MERGED` already means something other than this step merged it: stuck. Not `OPEN`: stuck.
2. The PR head and `review.md` must be current (see the state table), and `review.md` must be `pass`. Otherwise `done`, needing a human merge. The merge in step 7 matches the PR head, including the head from before a `BEHIND` update when `update-branch` reports none; this goes beyond the issue's literal text on purpose, so a rebase after review still merges the reviewed head.
3. Take the merge lock, an OS `flock` on `ns-merge.lock` in the common git dir, held until the step ends, so units merge one at a time per repo. While another unit holds it, the step prints `ns run: <unit> merge waits for the merge lock (held by pid <pid>, unit <unit>)`, logs a `lock_wait` event with `phase` and `lock` both `merge`, and tries again every 50 ms, in real time whatever `NS_NOW` says. A stop ends the wait like any other (see "Units a night didn't finish"). The wait has no timeout: a live holder's step is bounded by its own CI timeouts, and the kernel drops a dead one's hold. `ns watch --until` bounds it: the run ends as `budget` with `gave up waiting for the merge lock (held by pid <pid>, unit <unit>) at --until` and a `lock_wait_timeout` event with `bound: until`. Then `gh pr view <pr> --json mergeStateStatus`, so a PR is judged against the main the merge before it left. Right after its base moves GitHub reports `UNKNOWN` while it works the state out, so an `UNKNOWN` is read again with the backoff of step 4, for up to `ci_register_timeout` minutes, after which the step goes on as for any state other than `BEHIND` or `DIRTY`. Branch protection requires PRs to be up to date with the base. `BEHIND`: `gh pr update-branch <pr>`. `DIRTY`, or a failed update: back to build with "rebase onto <default> and resolve conflicts" as `{feedback}`, which uses a build attempt.
4. GitHub may not have registered checks on a head it just received, so the step first polls `gh api repos/{owner}/{repo}/commits/<sha>/check-runs` and `.../status` for the PR head (the one `update-branch` returned, if it ran) until either reports a check. Polls back off from 5 s to 30 s, bounded by `ci_register_timeout` minutes. None by then: `done`, needing a human merge, with reason "no CI checks registered within <n> min on PR #<pr>; needs a human merge". If the last poll's `gh api` call failed (auth, rate limit, 404) or printed something other than a count, the reason is instead "could not query CI checks on PR #<pr>: <error>; needs a human merge". Then `gh pr checks <pr> --watch`, killed after `ci_timeout_minutes` (stuck). Then `gh pr checks <pr> --json name,state,bucket,link`. No checks at all: `done`, needing a human merge. Any check not `pass` or `skipping`: back to build with the failing check names and the tail of `gh run view <id> --log-failed` as `{feedback}`, then verify, review and ship onto the same PR.
5. `gh pr diff <pr> --name-only`. Any path matching a `human_review` glob (`**` crosses directories): `done` with reason "changes files that need human review; needs a human merge". Files that need human review cover the factory's own guardrails: CI config, the definition, the guard and merge code.
6. Marked regions (below): HEAD is diffed against its merge base with `origin/<default>`. A changed line inside a region, in the base or the head version, or an added, removed or moved marker line: `done` with reason "changes code in a human-review region; needs a human merge (<file>:<start>-<end>: <reason>)". If the merge base can't be found, the reason says so and the unit still needs a human merge.
7. `gh pr merge <pr> --squash --delete-branch --match-head-commit <sha>`. If `gh pr view` then reports `MERGED`, the outcome is `merged`, and the unit is cleaned up once its quality records are saved (see "Cleanup").

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
ns watch [--once] [--until HH:MM] [--max-units N] [--parallel N] [--dry-run] [--factory <dir>]
```

The loop below is for one unit at a time, the default. With `--parallel N` the same steps run for up to N units at once; see "Parallel units".

1. Run the triage pass (see "Triage pass"), so issues filed tonight can join tonight's queue.
2. List open issues with `ready_label` via `gh api --paginate repos/{owner}/{repo}/issues`. The REST list carries each author's association, which `gh issue list --json` lacks.
3. Drop issues whose author is outside the team (see Trust), issues whose first-line `Blocked by: #a, #b` names any open issue, and issues that already have an open PR whose body says `Closes #n`, unless that PR comes from the issue's own unit branch (see "Units a night didn't finish").
4. Sort by the first matching `priority` label, then the first matching `order` label, then issue number. An issue with no `priority` label sorts after the last one.
5. Take the first. Swap `ready_label` for `in_progress_label`, so no other worker takes it. `git fetch origin`, then start `ns run --issue <n> --base origin/<default>` as a child process on the night's snapshot (see "Parallel units"), sharing the budget.
6. On `merged`, remove `in_progress_label`; GitHub closes the issue through the PR's `Closes #n`. On `done`, swap to `done_label`. On `split`, swap to `split_label` and post nothing: triage's notes already list the children, and the parent waits on them, neither queued nor triaged again, until a human closes it. On `stuck`, or a `done` that needs a human merge (files that need human review, no CI), swap to `stuck_label` and comment the reason and the last artifact path. When the last artifact is a `review.md`, the comment also lists its open Critical and Important findings, each with its title and location. The comment carries the AI disclaimer. On `budget`, put `ready_label` back and stop.
7. On `paused`, keep `in_progress_label` and sleep until the reset time (30 minutes when unknown, then check again), then resume the same unit. If the reset is at or past `--until`, put `ready_label` back and stop cleanly. With more than one unit running, a usage limit pauses them all (see "Parallel units").
8. Repeat until the queue is empty, `--until` passes (no new unit starts after it), `max_units` is reached, the budget is spent, or a signal asks it to stop.

At start, and again each time a unit ends (outside a usage-limit pause), before the triage pass, `ns watch` removes the worktrees of units whose issue has closed with their merged PR since: a PR a human merged after the unit ended `done`. It saves their quality records first and skips what `ns clean` skips (see "Cleanup"). A failure there is printed and never ends the night.

`--until HH:MM` is local time, from `TZ` or the system zone, so `06:30` means 06:30 where `ns watch` runs, across DST changes. Every time `ns watch` prints for a person (`until`, `reset_at`, the "sleeping until" line) is local, in RFC 3339 form with its offset: `2026-10-09T06:30:00-04:00`. The `ts` of each event in `runs.jsonl` stays UTC (`...Z`).

### Parallel units

`--parallel N`, or `[limits] parallel` (the flag wins; 0 is a usage error), runs up to N units at once. The aim is throughput where the queue allows it, not a busy machine: a slot with nothing ready to take stays idle. `parallel = 1`, the default, runs one unit at a time through the same machinery.

**Each unit is its own process.** `ns watch` starts every unit, and every resume of a paused one, as a child `ns run --issue <n>`, in its own worktree. A unit's state is frozen when it starts: its code and `docs/agents/` by its worktree, its binary by its process (on Linux, the binary `ns watch` started from, even if a newer one was installed since), and its config and factory definition by the night's snapshot, which the triage pass reads too. The snapshot is a private directory, `<git-common-dir>/ns/watch-<pid>/`, that `ns watch` writes at start and removes at the end (a dead watch's is removed by the next one): a copy of the factory directory (symlinks are left out, with a warning), a copy of the config file, which the unit gets as `NS_CONFIG`, and `night.json`, with the watch's pid, `--until` and the build gate command. Installed skills are not frozen: the skills sync runs before each unit starts, and a unit that is running sees a skill that changes under it, a trade-off the design accepts.

**The scheduler** fills free slots, records each unit that ends and refills:

- Filling: units paused on a usage limit resume first. Then `--max-units`, `--until` and the budget are checked, the triage pass runs, the queue is read once, and its first ready issues fill the free slots. Each is claimed, `ready_label` swapped for `in_progress_label`, before its `ns run` starts, and an issue running, paused or finished tonight is never taken again, even while GitHub still lists it as ready.
- Running: when a unit ends, `ns watch` records its outcome and labels the issue as in the loop above, then fills again, so the triage pass runs between finished units. An empty queue with units still running waits for them; the night ends `queue empty` only when nothing runs, nothing is paused and nothing is ready.
- Paused: a usage limit in any unit, or in a triage-only run, holds every unit. `ns watch` writes the reset time to the snapshot's `hold.json`, and each unit's `ns run` reads it before every phase, and again after waiting for runner locks: while the reset is still to come, the unit ends `paused` with reason `usage limit: another unit of this ns watch hit it` and `held: true` in its result, its artifacts untouched. A held unit resumes at the hold's reset, at once if that came while it ended; a unit that hits the limit itself waits for its own reset, or 30 minutes when that has passed. A phase already running finishes. Nothing new starts while the hold lasts. A later reset moves the hold later, and every paused unit resumes together at the latest one. A reset at or past `--until` stops the night, and every paused unit goes back to `ready_label`.
- Stopping: `--until`, the budget and the harness breaker start nothing new, not even a paused unit, and let running units finish. The budget is summed across units: every phase's cost, a unit's or a triage-only run's, is appended to the snapshot's `spend.jsonl`, and every unit checks the total before each phase, so a spent budget ends every running unit at its next phase. `--max-units` and `--once` claim nothing new but still resume paused units. A signal is passed on to every unit (see "Units a night didn't finish").

A unit that ends with no result (its `ns run` failed, as when the issue can't be read) goes back to `ready_label`; the other units finish, and `ns watch` prints the summary and exits with that run's exit code.

The triage pass and the cleanup run in `ns watch` itself, between units, as with one unit. Cleanup leaves the worktree of a unit running or paused alone, even one whose `ns run` hasn't taken its run lock yet. While they run, a unit that ends waits to be recorded and its slot to be filled, and a usage limit it hit reaches the other units only then.

Units share the repo, and its locks (D43) keep them apart: a unit's run lock, the worktree lock while a worktree is cut, the merge lock around each merge, and each runner's locks around its phases (see "Runners" and "Merge"). A unit waiting for a lock holds its slot.

`ns watch` prints `ns watch: #<n> <title> (worker <w>)` when worker `w` starts a unit (`, resumed` for a resume) and `ns watch: #<n> <outcome> (worker <w>)` when it ends; every unit's record in `units` has its `worker`, and the run log gets `worker_start` (`worker`, `issue`, `run_pid`, `resumed`) and `worker_end` (`worker`, `issue`, `unit`, `outcome`, `exit`) events. The unit's own events carry its `ns run`'s pid. Quality records are written by each unit's `ns run` when it ends, as for one unit.

With N above 1, set `[limits] memory_mb` (see "Memory cap"). Each unit's `ns run` wraps its own phases and gates in the cap, so N units can use up to N times `memory_mb`; pick a cap that leaves room for N of them. `--dry-run` shows the `parallel` it would use.

### Triage pass

Review files escapes and follow-ups as `needs-triage` issues, and only `ready_label` issues are queued, so before each unit `ns watch` triages what nobody has yet:

1. List every open issue (`gh api --paginate repos/{owner}/{repo}/issues?state=open`). A candidate carries `triage_label`, or no state label at all. A state label is any `status:` label or one of the `[queue]` labels. Issues whose author is outside the team, and issues an open or merged PR closes, are skipped, as in the queue. A blocked issue is still a candidate: triage can triage it, and the queue skips it until its blocker closes.
2. Drop issues already given a triage-only run tonight, whatever came of it, and issues whose unit ended tonight. Sort the rest by the first matching `priority` label, highest first, then by issue number. A candidate with no `priority` label sorts after the last one, and the queue's `order` labels play no part. An escape carries a priority from its severity, so it goes ahead of unprioritised issues.
3. Pick one. A candidate no earlier pass tonight listed is new: filed since the last pass, such as a follow-up or escape the last unit filed, or newly a candidate. The first new candidate goes first, whatever the queue holds. With none new, take the first of the rest, the backlog, but only while the queue has no ready issue (step 2 of the loop above, with its skips). Once one is ready, the pass ends and the backlog waits for a later pass. At the night's first pass nothing counts as new, so a big backlog never holds up the first unit: the pass triages until one issue is ready, or the backlog runs out.
4. Run it as a triage-only run: after the fetch and skills sync a unit gets, in process, `ns run --issue <n> --base origin/<default>` with the triage phase forced, even over an earlier `brief.md`, run once, and then stopped. It never sets `in_progress_label`, so triage applies the state and priority itself (ready-for-agent, needs-info, needs-define, wontfix, ...). The prompt ends with a `Triage only:` line telling the phase to stop after the outcome and hand off to no other phase. A ready-for-agent outcome writes `brief.md` in the unit's worktree, so the unit later starts at build.
5. Repeat from 1 until step 3 picks nothing, then pick the unit. The pass runs again before every unit, so issues the night's units file are triaged before the next pick, an issue triaged ready sorts into the queue by its priority, and the backlog goes on whenever the queue runs dry.

No count caps the pass: `--until` and the budget bound it, and every candidate is tried at most once a night. Floods are limited where issues are created, such as the audit's issues per run and one escape per review finding.

A triage-only run is `done` when the phase exits 0, and `stuck` when it exits non-zero, is killed, or times out. A run that can't start (the issue can't be read, the unit's worktree path is taken) is recorded as `error` and the pass goes on to the next candidate. When a run fails to start with the same error as the last one that did, the cause is global (a missing login, an unwritable lock dir), so the pass is off for the rest of the night and the unit's own run reports the error. Either way the issue keeps the labels triage left, gets no comment from `ns watch`, and is not tried again that night. A run whose every attempt failed instantly counts toward the same harness breaker as units: two such runs in a row, triage-only or unit, stop the night with `harness failing`. A usage limit sleeps until the reset like a unit, then the pass picks again, normally the same issue, since the limit stopped it before it triaged; a reset at or past `--until` stops the night with no label change. `--until` and the budget are checked before every triage-only run. Triage-only runs are not units: `--once` and `max_units` count units only, so `--once` triages, then builds one unit.

Under `gates = "stop"` triage only recommends and applies nothing, so the pass is off (`--dry-run` shows `triage_pass` as false and no candidates). Every triage-only run creates the unit's worktree and branch; one that doesn't end ready-for-agent leaves them for the next attempt, like a stuck unit. Only `status:` labels and the `[queue]` labels count as states, so in a tracker whose other states lack the `status:` prefix (a bare `needs-info`), those issues look unlabelled and are triaged again each night.

Before each unit, after the fetch, `ns watch` keeps installed skills current. A unit's worktree already has the skills checked into the repo at `origin/<default>`, but a skill installed as a symlink into the main checkout (`~/.agents/skills/*`, `~/.claude/skills/*`) would otherwise run whatever the checkout held when the night began. So when any installed symlink resolves to a skill dir (one with a `SKILL.md`) in the main checkout, and that checkout is on the default branch with no changes to tracked files, `ns watch` fast-forwards it to `origin/<default>` with the checkout's git hooks off. It then reruns `ns install` for each dir of `ns-*` skills it found, so a skill added tonight is linked and a removed one is pruned. It logs all this to stderr and as a `skills_synced` event (`from`, `to`, `skills`, `relinked`) in the run log. On another branch, a detached HEAD, local changes or diverged history, it changes nothing: it prints a warning naming the reason and the skills, logs a `skills_stale` event, and runs the unit anyway. The same reason warns once, not before every unit. It never resets, stashes or switches branches. Skills installed from anywhere else, such as a consumer's nightshift checkout, are never touched.

An issue whose `Blocked by:` issue can't be read counts as blocked. `NS_NOW` (unix seconds) pins the clock for tests; sleeps then advance it instead of blocking. Output: `{units:[{issue, unit, worker, outcome, reason, pr, cost_usd}], triaged:[{issue, unit, outcome, reason, cost_usd, state}], requeued, cleaned, stopped, until, cost_usd, started_with, error}`, where `cleaned` lists the units whose worktrees the night removed after a human merge; `until` is null without `--until`, and `error` is null unless the night ended on one (`stopped` is then `error`, and `ns watch` exits with that error's code after the summary; a signal that came too names `stopped` and the exit code instead). A unit record that ran has its `worker`. A triaged record's `state` lists the issue's state labels after the run (null when they couldn't be read); a paused one has `reset_at` instead, and an `error` one only `issue`, `outcome` and `reason`.

`ns watch` reads the user config and the factory definition once, at start, and copies both into the night's snapshot, which every unit's `ns run` reads. Editing or breaking either file mid-night changes nothing until the next `ns watch`. The build gate command is read at start too, so a `docs/agents/stack.md` that a fast-forward of the main checkout brings in (above) takes effect at the next `ns watch`. It logs each file's path and sha256 to stderr, and `started_with` carries the same `{config, factory}` pair of `{path, sha256}` (`sha256` is null for a missing file).

`--once` takes one unit. `--dry-run` prints the ordered queue, with each issue's `priority_label` and `order_label`, and the skip reasons, then whether the triage pass runs (`triage_pass`), the triage candidates in triage order (`triage`, each with why it needs triage and its `priority_label`), their skip reasons (`triage_skipped`), and `triage_next_pass`: the candidates the next pass would take, in order. A dry run is a fresh night, so every candidate is backlog: with an issue ready, or one in `requeue`, which the night returns to the queue before its first pass, the list is empty, and with none it is every candidate, which the pass takes until one is triaged ready. `gh` and the phases get `GH_TOKEN` from `[forge.github]` in the user config (`cli/README.md`, `[forge]`). A `GH_TOKEN` already set in the environment wins, so whoever starts `ns watch` can still pick the account, and the units cleanup would remove (`clean`).

### Units a night didn't finish

`ns watch` holds a watch lock, an OS `flock` on `ns-watch.lock` in the common git dir, for its whole run, so a second `ns watch` on the repo exits 5 (`another ns watch (pid <pid>) is running`). `--dry-run` takes no lock.

At start, before the triage pass, it lists open issues carrying `in_progress_label`. A night that crashed, was killed with SIGKILL, or lost its terminal leaves its unit's issue there. An issue that a live `ns run` holds through a run lock (the lock's `issue`, or a unit id `<n>` or `<n>-...`, read from the file name while the holder hasn't written its record) is left alone, with `ns watch: #<n> is held by a live ns run (pid <pid>, unit <unit>); left in progress` (pid `?` before the record is written). Every other one goes back to `ready_label`, with `ns watch: #<n> was in progress with no live ns run; back to <ready_label>` on stderr and a `requeued` event in the run log. Its worktree, branch and `.ns/<unit>/` artifacts stay, so the next run resumes the unit where the state table says. The summary's `requeued` lists these issues, and `--dry-run` lists the ones it would return as `requeue`. `in_progress_label` belongs to `ns watch`: an issue a person marks in progress by hand, or one another machine's `ns watch` is running, has no local lock and is returned too.

On SIGINT (Ctrl-C) or SIGTERM, `ns watch` starts no new unit, triage run or phase, and ends the work in hand:

- Each running unit's `ns run` gets the signal on its process group, which also ends a setup command or `gh` call in flight there. Its running phase, gate or CI wait then gets SIGTERM on its own process group, then SIGKILL 5 s later. A unit's `ns run` has a process group of its own and the phase a session of its own, so a Ctrl-C at the terminal reaches only `ns watch`, which passes it on this way.
- When the phase, or the gate run after it, is cut short, what the phase wrote to its artifact moves to `.ns/<unit>/history/<artifact>-interrupted-<n>.md` and the artifacts archived before it started are put back, so the next run runs that phase again. The run log gets a `phase` (or `gate`) event with `interrupted: "SIGINT"` or `"SIGTERM"`, and an `end` event with outcome `interrupted`.
- A usage-limit sleep or a lock wait ends within a second.
- `ns watch` waits for every unit's `ns run` to end. Each unit's issue goes back to `ready_label`, as does that of a unit paused on a usage limit, and its record in `units` has outcome `interrupted`. A record for a run cut short carries only `issue`, `outcome` and `reason`. A run that ended on its own while the stop was asked for keeps its outcome only when it is `merged`, `split` or a clean `done`. Any other outcome may come from the stop (Ctrl-C also kills a `gh` call in flight), so the issue goes back to the queue instead.

Then it prints the summary with `stopped` set to `SIGINT` or `SIGTERM` and exits 128 + the signal (130 or 143). A unit's `ns run` stopped by a signal `ns watch` didn't pass on stops the night the same way. Each of `ns watch`'s handlers runs once: a second signal of the same kind ends `ns watch` at once, as before, without returning the issues, and the next start returns them. A unit's `ns run` only records each signal, since a service manager may signal it as well as `ns watch` does; on Linux it gets SIGTERM when `ns watch` dies, so it sets its phase aside and ends rather than run on alone. SIGHUP and SIGKILL aren't handled: the next `ns watch` returns that unit at start, as above.

A unit stopped after ship has its own open PR, which closes the issue. The queue skips an issue an open PR closes, except a PR from the issue's own unit branch: a branch of this repo, not a fork, named `ns/<unit>` for the unit `ns run --issue <n>` would run (`<n>-<slug of the current title>`). That unit is queued again and `ns run` resumes it at the merge step.

## Unit lifecycle

A unit is one issue's work, from the issue to its merged change. It has the issue, a worktree `<repo>.worktrees/<unit>` on the branch `ns/<unit>`, its artifacts in the worktree's `.ns/<unit>/`, a PR, events in the run log, and quality records.

1. **Queued.** Triage labels the issue `ready_label`. When `ns watch`'s triage pass does it, the triage-only run has already created the worktree and branch and written `brief.md` there.
2. **Started.** `ns watch` swaps the label for `in_progress_label` and runs `ns run --issue <n>`. That creates the worktree from `origin/<default>`, or reuses the one an earlier run left, and runs `[worktree] setup`.
3. **Phases.** Triage, build, verify, review and ship run in the worktree, each writing its artifact to `.ns/<unit>/`. Ship pushes `ns/<unit>` and opens the PR, whose body closes the issue.
4. **Ended.** The unit ends `merged` (the merge step merged it), `done` (a human merges), `stuck`, `split`, `paused` or `budget`. On `merged`, `done` and `stuck`, `ns run` writes the unit's quality records.
5. **Cleaned up.** Once the change merges and the records are saved, the worktree, its artifacts and the local branch are removed. `ns run` does it right after its own merge. `ns watch` does it at start and between units for a PR a human merged. `ns clean` does it on demand. The remote branch goes with the merge: the merge step passes `--delete-branch`, and for a human merge the repo's setting decides.

After cleanup, the run log and transcripts under the git common dir, the quality records on `nightshift/quality`, the PR and the issue remain. A unit whose issue is still open keeps its worktree, so the next run resumes it: a stuck unit, a paused or interrupted one, parked work, or a triage-only run that ended needs-info or needs-define.

### Cleanup

```
ns clean [--dry-run]
```

A unit's worktree goes when its change merged. For `ns run`, that is its own merge step reporting `MERGED`. For `ns watch` and `ns clean`, all of these must hold:

- The unit id starts with an issue number (`142-uart-timeout`), and that issue is closed.
- `pr.md` (or its newest archived copy) names a PR, and `gh pr view` reports it `MERGED`.
- The PR's body closes the issue (`Closes #n`, or fixes or resolves), and its head branch is `ns/<unit>`.

Its quality records are saved first, with outcome `merged`, all of the sweep's units in one push. A record that lands in the outbox counts as saved: the outbox keeps it until the next push. A record that can't be saved at all keeps the worktree, and so does a review artifact that can't be read (no frontmatter, say), since no record holds it.

Cleanup never removes:

- A worktree with uncommitted changes to tracked files. Untracked and ignored files, such as build output and `.ns/`, go with the worktree.
- A worktree whose HEAD is not on `ns/<unit>`: detached, or on another branch.
- A worktree whose branch holds changes the merged PR's head doesn't. The head holds the branch when it is the branch's tip, descends from it (`update-branch` merged the base in), or carries the same change against the default branch, whitespace included (`git patch-id --verbatim`, as after a rebase). Currency ignores whitespace; cleanup doesn't, because a re-indented line can change what code does. A head missing locally is fetched from `refs/pull/<n>/head` first, with the same timeouts as the quality push. One that still isn't there keeps the worktree.
- A unit whose run lock (`ns-run-<unit>.lock`) a live `ns run`, or another cleanup, holds. Nothing is recorded for it either. `ns watch` and `ns clean` take each unit's run lock before they record it, and hold it while they check the worktree again and remove it, so no run starts on the unit meanwhile. `ns run` already holds it when it cleans its own unit. Only `git branch -D` runs under the worktree lock, since it edits `.git/config` as `git worktree add` does; deleting the tree doesn't hold up other units' worktrees.
- The main checkout, or a worktree the command runs from.

`ns watch` tries each merged unit once a night. One that stays, say because `git worktree remove` failed, is tried again by the next night or by `ns clean`, so its records are written once a night at most.

Removal is `git worktree remove --force` (a locked worktree still refuses), then `git branch -D ns/<unit>`. Each attempt logs a `cleanup` event with `unit`, `path`, `branch`, `issue`, `pr`, `removed`, `reason` when it stayed, `error` when a git command failed, and `by` (`run`, `watch` or `clean`).

`ns clean` reads every worktree on an `ns/` branch, the main checkout included, and prints `{ok, command, dry_run, status, removed, kept, records}`. `kept` lists every unit that stays, with its reason. `records` is the quality write's event. `status` is `cleaned`, `planned` (a dry run with something to remove), or `nothing-to-clean`, so a rerun is safe. `--dry-run` prints `planned` instead of `removed` and writes nothing: no record, no event, no removal. It still reads issues and PRs with `gh` and may fetch a PR head. It exits 1 when a removal failed.

## Quality

```
ns quality [--since YYYY-MM-DDTHH:MM:SSZ|YYYY-MM-DD] [--json]
ns quality import <dir> [--dry-run]
```

`ns quality` measures whether build writes code that passes review. It reads each unit's quality record first, and for a unit with no record, its review artifacts in the linked worktrees (`.ns/<unit>/review.md`, `review/cycle-*.md`, and archived `history/review*`). It adds the run log and prints JSON. The main checkout is skipped. First-pass numbers come from a unit's oldest review artifact, since a rebuilt unit's later review starts from a different diff. Everything else comes from the newest.

| Metric | What it counts | Target |
|---|---|---|
| First-pass yield | Units whose first review pass raised no Critical or Important finding in changed code, over units where that is known | Rising |
| Findings per 100 lines | Critical and Important findings in changed code from the first pass, per 100 changed lines, overall and per axis. A finding with two axes counts once in each | Falling |
| Cycles to clean | Fix cycles a unit needed to end `pass` with no open Critical or Important in changed code (median, max). Units that never got there are `not_clean` | Falling |
| Leftovers | Critical and Important findings in changed code still `open` after the third fix cycle | 0 |
| Escapes | Findings with `Scope: pre-existing` (D29): defects in code the unit didn't change, so an earlier unit's review let them through. Each is blamed (`git blame` at the reviewed commit, the frontmatter's `sha:`, else `head:`, else `base:`) to the commit that introduced the line, and to the PR in its subject (`(#123)`) | Falling |

An escape counts only as an escape: it never makes a unit's first pass dirty, never counts toward findings per 100 lines, and never blocks. A finding dismissed in review counts toward none of these: review judged it wrong. `trend` repeats the numbers for each local day (the newest artifact's `updated:`), and `--since` keeps only units whose newest artifact was updated, and run-log events, at or after a UTC instant. A bare date means 00:00:00 UTC, whatever `TZ` is, and an undated unit or event is dropped. `per_unit` has each unit's numbers, with `changed_lines`, which keeps the review cost of a large unit visible now that size never blocks one (D37), `reached_clean` (`clean`, `not_clean` or `unknown`) and its review runs, review cost and outcome from the run log. `run_log.units_without_artifacts` names units the log shows reviewed that have neither a record nor a worktree.

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

### Where the history lives

Worktrees get removed once their change merges (see "Cleanup"), so the history lives in the repo, on an orphan branch `nightshift/quality` on `origin` (D39). It holds one file, `records.jsonl`, with one JSON line per review attempt. When a unit ends `merged`, `done` or `stuck`, `ns run` writes the unit's records and pushes them. Each record has the unit, issue, PR, outcome, the attempt's status, `updated:`, local day, fix cycles and change size, and each finding's id, severity, axes, scope, cycle, status and relative location. Finding titles are left out. Paths are relative, and a record holding an absolute path is never written. A location with one is dropped.

`ns run` fetches the branch, appends on its tip and pushes. If origin rejects the push because another writer pushed first, it fetches and tries again, up to 3 more times. Any other failure (no `origin`, no access, a hook that declines the push, the network) goes straight to the outbox, as does a fourth lost race. The records wait in `<git-common-dir>/ns/quality-outbox.jsonl`, and the next `ns run` or `ns quality import` pushes them. A unit never fails because its record didn't push. The branch is data: `ns` writes it unattended, outside `human_review` and the PR flow. Commits are built with git plumbing, so writing one never touches a working tree, the index or a local branch. These git calls never prompt: credential prompts are off, ssh runs in batch mode with connect and keepalive timeouts unless you set your own ssh command, and an HTTP transfer under 1000 bytes a second for 60 seconds fails. That matters because a write holds a lock on the outbox while it talks to `origin`.

`ns quality` fetches the branch and reads `records.jsonl` plus the outbox. A unit written more than once (stuck, then rerun) counts from its newest write, the latest `recorded` time. A unit's record wins over its worktree, and a unit with no record falls back to its worktree. So the numbers stay the same after `ns worktree remove`. `per_unit.source` says which one each unit came from (`record` or `worktree`), and `records` says how many records came from the branch and the outbox, and why a fetch failed. An escape is blamed when its record is written, while the reviewed commit still exists, and the record keeps the commit and PR (`introduced_by`). An older record without one is blamed in the main checkout at the recorded commit. An escape from a record has no title.

To read the trend, run `ns quality` from any clone and look at `trend`: one summary per local day, oldest first. First-pass yield should rise and findings per 100 lines should fall. To see the raw records, run `git show origin/nightshift/quality:records.jsonl`.

`ns quality import <dir>` turns archived unit worktrees into records and pushes them in one commit. `<dir>` holds one directory per unit, laid out as in its worktree (`<dir>/<unit>/.ns/<unit>/review.md`). It skips units the branch or the outbox already has, so a rerun reports `already-done`. Issue numbers come from the unit id, PRs from `pr.md` or the run log, and outcomes from the run log. `--dry-run` fetches the branch to see what is recorded, and pushes nothing.

## Trust

Phases run unattended with permissions bypassed, and on a public repo anyone can open an issue or comment on one. The team is the authors whose association is `OWNER`, `MEMBER` or `COLLABORATOR`.

- **Only team-authored issues are queued.** `ns watch` skips any other issue with reason `author outside the team`. A missing association counts as outside the team.
- **Only team comments are instructions.** Phases quote other text as data and never act on it. The rule lives in `skills/ns-contract/SKILL.md` under "Untrusted issue content", and the skills that read issues point there.

What remains: the team's own text is trusted in full, so a team member who pastes untrusted text into an issue or comment passes it through as instructions. `ns run --issue <n>` started by hand runs any issue, whoever wrote it, and only the skills' rule guards its comments.

## Starting a night

Run `ns watch` from the systemd user timer in `contrib/systemd/`, so it gets a service of its own. A terminal or a tmux session works too, but then the watcher shares its fate with whatever else runs there. On 2026-10-10 the kernel OOM-killed one test binary, systemd stopped the tmux scope it ran in, and that took `ns watch` and the night down with it (#208).

The phases run with permissions bypassed inside worktrees. Run on a machine where that's acceptable.

### Before the first night

1. Log in to claude with the subscription account (`claude /login`). Without the login `ns run` refuses to start, with exit 2 (see "Billing").
2. Install `ns` and the skills from a nightshift checkout: `cargo install --path cli`, then `ns install`.
3. Put the account PRs should come from in the user config, `~/.config/nightshift/config.toml`, and run `ns doctor`. Its `forge` entry should show `token_resolved: true` and that account.

   ```toml
   [forge.github]
   token_command = "gh auth token --user <account>"
   ```

4. Keep the watched checkout on the default branch with no changes to tracked files. `ns watch` reads the gate command from it at start. When your skills are symlinks into it, `ns watch` also fast-forwards it before each unit so they stay current; on another branch or with local changes it only warns, and the night runs on the old skills. Skills linked from a separate nightshift checkout are never updated by `ns watch`, so pull that checkout yourself.
5. Run `ns watch --dry-run` in the checkout to see the queue it would work.

### Install the timer

```bash
mkdir -p ~/.config/systemd/user
cp contrib/systemd/ns-watch.service contrib/systemd/ns-watch.timer ~/.config/systemd/user/
systemctl --user edit ns-watch.service
```

The unit runs in `~/src/myrepo`. Point it at your checkout in the drop-in that `systemctl --user edit` opens. To change the end of the night, clear `ExecStart` and `ExecCondition` with an empty line each and set them again. Both are lists, so a drop-in that only adds a line leaves the packaged one running too:

```ini
[Service]
WorkingDirectory=%h/src/myrepo
ExecCondition=
ExecCondition=/bin/sh -c 'h=$$(date +%%H); [ "$$h" -ge 21 ] || [ "$$h" -lt 7 ]'
ExecStart=
ExecStart=%h/.cargo/bin/ns watch --until 08:00
```

Then enable the timer, and let your user manager run while you are logged out:

```bash
systemctl --user daemon-reload
systemctl --user enable --now ns-watch.timer
loginctl enable-linger "$USER"
```

The timer starts the service at 22:00 local time. To start a night earlier, after 21:00, run `systemctl --user start ns-watch.service`. Follow it with `journalctl --user -u ns-watch -f`, which shows `ns watch`'s stderr and, at the end, its JSON summary. Watch the first night start this way: `ns doctor` in a terminal checks the terminal's environment, not the service's.

`--until` is local time, from `TZ` or the system zone, and means the next 06:30. A night that starts at 08:00 would run until 06:30 the next day, so the units guard against late starts:

- The timer has no `Persistent=`, so a 22:00 missed while the machine was off is skipped, not run at boot.
- A 22:00 missed while the machine was suspended does run on resume; systemd catches up calendar timers after a suspend. The service's `ExecCondition=` skips any start between 06:00 and 21:00, so that one is skipped too. It skips a daytime `systemctl --user start` as well: for a daytime run, start `ns watch` in a terminal. If you change `--until`, change the hours in `ExecCondition=` to match, as in the drop-in above.

Only one `ns watch` runs per repo. A second one, from a terminal or another unit pointed at the same checkout, exits 5 with `another ns watch (pid <pid>) is running`. The timer never starts the service twice, because systemd doesn't start a unit that is already running.

The service gets the token through the config's `token_command`, as in a terminal. If that command can't run under systemd, for example because gh keeps the token in a desktop keyring that stays locked while you are logged out, put the token in `~/.config/nightshift/ns-watch.env` instead, outside any repo, and `chmod 600` it:

```bash
GH_TOKEN=<token>
```

The unit reads that file when it exists, and a `GH_TOKEN` in the environment wins over the config.

The service has no SSH agent either, so `git fetch` and `git push` over an SSH remote whose key lives in the agent fail on every unit. Use an HTTPS `origin` and run `gh auth setup-git` once, so git authenticates with `GH_TOKEN`, or use a key that needs no agent.

What the other settings in `ns-watch.service` do:

- `OOMPolicy=continue` keeps the service running when the kernel OOM-kills one of its processes. The default for a service is to stop it, which is the 2026-10-10 failure again. The killed phase or gate fails its attempt, and the night goes on.
- `KillMode=mixed` sends `systemctl --user stop`'s SIGTERM to `ns watch` alone. `ns watch` then starts nothing new, sends SIGTERM to the running phase's process group and SIGKILL 5 s later, moves the phase's partial artifact aside, returns the issue to the ready label and exits 143 (see "Units a night didn't finish"). The default, `KillMode=control-group`, would send SIGTERM to every process in the service at the same moment, the phase and any `git` or `gh` call included, so the shutdown would no longer go in that order. If `ns watch` hasn't exited after `TimeoutStopSec=2min`, systemd sends SIGKILL to everything in the service. Two minutes covers the 5 s grace and the `gh` calls that relabel the issue.
- `SuccessExitStatus=143` records a stop as a clean exit, not a failure.
- `MemoryHigh=` and `MemoryMax=` are commented out. Set them to cap the memory of the whole night, `ns watch` included. With `MemoryMax=` the kernel OOM-kills inside the service before the machine runs short, and `OOMPolicy=continue` keeps the night going. They need cgroup v2. A per-phase memory cap in `nightshift.toml` is planned in #208. `OOMPolicy=` covers only the kernel's OOM killer: where systemd-oomd runs, it can kill the whole service under memory pressure, and a `MemoryHigh=` that throttles the night before the machine runs short makes that less likely.
- `PATH` lists `~/.cargo/bin` and `~/.local/bin`, since a user service doesn't read your shell profile. Add the directories where `claude` and `gh` live if they are elsewhere.

### When the watched repo is nightshift

The night runs the `ns` binary that was installed when it started. Units that merge changes to `cli/` don't reach it until the next build. Before each night, pull the default branch and rebuild, here with the nightshift checkout as the watched one:

```bash
git -C ~/src/nightshift pull --ff-only
cargo install --path ~/src/nightshift/cli
```

To have the service do it, add this to its drop-in. A failed pull or build then fails the start, and the night doesn't run on a stale binary:

```ini
[Service]
WorkingDirectory=%h/src/nightshift
TimeoutStartSec=30min
ExecStartPre=git pull --ff-only
ExecStartPre=%h/.cargo/bin/cargo install --path cli
```

`git pull` here runs before `ns`, so it doesn't get the `GH_TOKEN` that `ns` resolves from `token_command`. A public repo needs no token. For a private one, put `GH_TOKEN` in `ns-watch.env`, which `ExecStartPre=` reads too, with the HTTPS remote and `gh auth setup-git` from above.

#156 will let `ns watch` update its own binary between units, and this step goes away.

### Stopping a night

`systemctl --user stop ns-watch.service`, or Ctrl-C when `ns watch` runs in a terminal, ends the night early. Every unit in progress goes back to the ready label and resumes on the next night. A second Ctrl-C ends `ns watch` at once, without returning the issues; each unit's `ns run` then ends its phase too. Under systemd a second `stop` sends nothing more; `systemctl --user kill --kill-whom=main ns-watch.service` sends the second SIGTERM (without `--kill-whom=main` it signals the phase too). That, or the SIGKILL at `TimeoutStopSec`, leaves the issue in progress, and the next `ns watch` returns it at start.

### In the morning

Start with the summary at the end of the journal:

```bash
journalctl --user -u ns-watch --since yesterday
```

The run log, `.git/ns/runs.jsonl` in the main checkout's git dir, has one JSON line per event. Its `ts` is UTC. This lists how each run ended, in local time, triage-only runs included:

```bash
jq -r 'select(.event == "end")
  | [(.ts | fromdate | strflocaltime("%a %H:%M")), .unit, .outcome, .reason]
  | @tsv' .git/ns/runs.jsonl
```

A `phase` event has the attempt and a `transcript` path for the phase's full output. One that ran to the end also has `exit`, `timed_out` and `cost_usd`; one cut short by a stop has `interrupted` and `archived` instead. `gate` events carry the CI gate's `log`. To see one unit's phases, filter with `select(.unit == "<unit>")`.

`ns quality --since` takes a UTC instant too. For the night that started at 22:00 yesterday:

```bash
ns quality --since "$(date -u -d "@$(date -d 'yesterday 22:00' +%s)" +%FT%TZ)"
```

The summary's `cleaned` lists the units whose worktrees the night removed after a human merged their PR; a unit the merge step merged was cleaned by its own run. Each attempt is a `cleanup` event, and one that stayed carries its `reason`:

```bash
jq -r 'select(.event == "cleanup") | [.unit, .removed, .reason // ""] | @tsv' .git/ns/runs.jsonl
```

On the tracker, a merged unit's issue is closed. Under `merge.policy = "human"`, a unit that ended `done` has `done_label` (`status:in-review` by default) and an open PR for you to merge. A stuck unit, or a `done` that needs a human merge, has `stuck_label` (`status:ready-for-human` by default) and a comment with the reason and the last artifact.

### Resuming a unit

A unit's worktree, branch and `.ns/<unit>/` artifacts stay after it stops, so it resumes where the state table says. They go only once its change merges (see "Cleanup").

- **Paused on a usage limit.** `ns watch` sleeps until the limit resets and resumes the same unit. If the reset falls at or after `--until`, it returns the issue to the ready label, and the next night picks it up. To resume it sooner, after the limit resets, run `ns run --issue <n>` in the checkout. Do that only when no `ns watch` is running: while one sleeps on the limit it holds no run lock, so nothing stops two runs working the same unit. `ns run` changes no labels, so set the issue's label yourself afterwards.
- **Interrupted by a stop.** The issue is back on the ready label. The next run runs the cut-short phase again, and what that phase had written is in `.ns/<unit>/history/<artifact>-interrupted-<n>.md`.
- **Lost to a crash, a SIGKILL or a lost terminal.** The issue keeps `status:in-progress`. The next `ns watch` returns it to the ready label at start and lists it in `requeued`.
- **Stuck.** Read the comment, fix what it names, and put the ready label back. `ns run --issue <n>` by hand also gives each phase fresh attempts, and leaves the labels to you.

To run `ns watch` in a terminal instead, keep its log somewhere you can read in the morning:

```bash
ns watch --until 06:30 >> ~/.local/share/nightshift/watch.log 2>&1
```
