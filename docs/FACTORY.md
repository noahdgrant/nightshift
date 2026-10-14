# Factory definition and runner

The minimum slice of D15 to D17 (`docs/decisions/`) that runs nightshift unattended: a definition in `.nightshift/`, `ns run` to drive one unit through the phases, and `ns watch` to pull units from the tracker overnight. Scorers grade finished units with `ns score`; acting on their scores and automations come later (#7).

## Layout

```
.nightshift/
  nightshift.toml          the definition root
  agents/<role>/agent.md   one role per phase (optional: built-in default prompts exist)
  runners/<name>.toml      exclusive locks a phase holds (optional; see "Runners")
  scorers/<name>.md        a rubric that grades finished units (optional; see factory/scorers.md)
  ratchets.json            recorded values of the [ratchets.*] tables (see factory/run.md#ratchets)
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
gate = "scripts/ci-local.sh"       # build only. Unset: the ci-local row of docs/agents/stack.md. See factory/run.md#gate
[phases.verify]
skill = "ns-verify"
runner = "bench"                   # a runners/<name>.toml. Unset: no locks
[phases.review]
skill = "ns-review"
[phases.ship]
skill = "ns-ship"

[queue]
source = "github"                  # github or gitlab. Unset: from origin's host. See below
ready_label = "status:ready-for-agent"
in_progress_label = "status:in-progress"
done_label = "status:in-review"    # set when the unit ends with an open PR
human_merge_label = "needs-human-merge"   # set on that PR; created in red if missing
stuck_label = "status:ready-for-human"
split_label = "status:split"   # set when triage split the issue into child issues; created if missing. See factory/watch.md#split-parents
triage_label = "status:needs-triage"   # issues ns watch triages before it picks a unit
priority = ["priority:high", "priority:medium", "priority:low"]  # sorted on first; no match goes last
order = ["type:fix", "type:feat", "type:refactor", "type:test", "type:docs", "type:chore"]
fix_main_first = true              # while main's CI is red, work only its fix issue. See factory/watch.md#mains-ci
overlap_ignore = []                # globs of files no two units overlap on. See factory/watch.md#parallel-units

[limits]
max_units = 8                      # optional cap per `ns watch` invocation; unset means no cap
budget_usd = 25.0                  # soft cap, summed from harness cost reports. Unset: no cap
                                   # under billing = "subscription", 25.0 under "api"
memory_mb = 16384                  # optional cap on each phase and gate, in MiB. Unset: no cap.
                                   # See factory/run.md#memory-cap
parallel = 2                       # units ns watch runs at once; --parallel wins. Unset: 1.
                                   # See factory/watch.md#parallel-units

[merge]                            # absent table: policy = "human"
policy = "auto"                    # auto | human. See factory/merge.md
human_review = [".github/**", "scripts/check-private.sh", ".nightshift/**", ".githooks/**"]
ci_timeout_minutes = 30
ci_register_timeout = 3            # minutes to wait for the PR head's first check before "no CI"
local_queue = true                 # gate the merge with the default branch before merging (the
                                   # default; off under [gate] local = "build-only").
                                   # See factory/merge.md#local-merge-queue

[gate]
local = "build-only"               # every-step (the default) | build-only. See factory/run.md#gate
                                   # on a personal-account repo build-only tests the PR head,
                                   # not the merge: see factory/merge.md#github-merge-queue

[supervise]                        # read by the ns-supervise skill only. See factory/starting-a-night.md#supervising-a-night
notify = "notify-send nightshift"  # optional command each ping is also sent to. Unset: none
silence_minutes = 45               # minutes with no run-log event, while units run, before a ping

[ratchets.flash]                   # a measured value that may never get worse. See factory/run.md#ratchets
command = "scripts/flash-size.sh"  # prints one number on stdout
better = "lower"                   # lower | higher
tolerance = "0.5%"                 # of the recorded value, or an absolute number. Unset: 0
```

`[queue] source` names the forge `ns watch` reads issues and PRs from and keeps labels and comments on. `github` uses `gh`; `gitlab` uses `glab` (its `api`, `issue update`, `issue note` and `issue close`), with `GITLAB_TOKEN` and `GITLAB_HOST` from `[forge.gitlab]`. Unset, `origin`'s host decides: a host containing `gitlab`, or equal to `GITLAB_HOST` (which `[forge.gitlab] host` sets), means `gitlab`, and anything else, including no `origin` or a local path, means `github`. A set value wins over `origin`. `ns watch` refuses an unknown value with exit 2 and a message naming `[queue] source`, and `ns factory validate` reports one. On GitLab only `ns watch`'s queue, labels and comments go through `glab`: `ns run`'s merge step still uses `gh` (#57). An error from `glab` shows its stderr with the token replaced by `[token]`.

Every table and key is optional; the defaults are the values above, except `queue.source` (unset: from `origin`, above), `limits.max_units` and `limits.memory_mb` (unset: no cap), `limits.parallel` (unset: 1), `limits.budget_usd` and `merge` as noted, and `supervise.notify` (unset: none). A phase table may override `harness`, `model`, `timeout_minutes` and `max_attempts`, may set `runner`, and its `skill` defaults to `ns-<phase>`. Only `[phases.build]` may set `gate`, and not to an empty string. `supervise.notify` may not be empty either, and `supervise.silence_minutes` must be at least 1. A `[ratchets.<name>]` table needs `command` and `better`; a negative or unreadable `tolerance` is an error. Unknown keys and unknown phases are errors. One removed key is not: `[queue] triage_per_night` still loads, does nothing, and gets a warning, on stderr from `ns run` and `ns watch` and in the `warnings` list of `ns factory validate`, which still passes. A file that set it to 0 to turn the triage pass off now runs the pass, and the warning says so: only `gates = "stop"` turns it off. `merge.policy = "auto"` with an empty `merge.human_review` gets a warning in the same places, since every PR then merges with no human, changes to CI config, hooks and `.nightshift/**` included. `ns-setup` proposes a starter list. `ns factory validate` checks the file, every `runners/<name>.toml`, every `agents/<role>/agent.md` (role matches the directory, placeholders are known) and every `scorers/<name>.md` (see ["Scorers"](factory/scorers.md)), lists the valid scorers under `scorers`, prints the `[supervise]` settings with their defaults under `supervise`, and exits 1 on any problem.

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

**A held lock makes the run wait**, not refuse. It prints `ns run: <unit> <phase> waits for lock <name> (held by pid <pid>, unit <unit>)` to stderr, logs a `lock_wait` event, and tries again with a backoff of up to 5 s until the holder lets go. The wait is bounded by the waiting phase's own timeout, counted from when the wait began, and by `ns watch --until`, whichever comes first; a hung holder, or a harness left running by a killed `ns`, can hold the lock for longer than its own run's timeout. On giving up, the run releases the locks it already took, prints `ns run: <unit> <phase> gave up waiting for lock <name> (held by pid <pid>, unit <unit>) at the phase timeout` (or `at --until`), and logs a `lock_wait_timeout` event with the lock, the holder and the `bound` (`timeout` or `until`). The phase timeout counts as a failed attempt, retried within `max_attempts`, and the out-of-attempts reason names the lock. `--until` ends the run as `budget` with that reason, and `ns watch` stops the night with `until`. A lock taken at or past `--until`, or taken with the budget spent, is released the same way (as `budget`), so the phase doesn't start after the deadline or on a spent budget. Exit 5 still means only the unit's run lock (see [ns run](factory/run.md)) or, for `ns watch`, its watch lock. The lock is an OS `flock` on the file, so the kernel drops a run's hold when its process dies, and a file naming a dead pid is taken over at once. If `ns` is hard-killed (SIGKILL or a crash), the lock frees at once while its harness child may still be running, since the child does not inherit the lock.

## Scorers

See [Scorers](factory/scorers.md): [Scoring](factory/scorers.md#scoring).

## ns run

See [ns run](factory/run.md): [Pinned skills and phase settings](factory/run.md#pinned-skills-and-phase-settings), [Gate](factory/run.md#gate), [Rules](factory/run.md#rules), [Ratchets](factory/run.md#ratchets), [Memory cap](factory/run.md#memory-cap).

## Merge

See [Merge](factory/merge.md): [Local merge queue](factory/merge.md#local-merge-queue), [GitHub merge queue](factory/merge.md#github-merge-queue), [Marked regions](factory/merge.md#marked-regions).

## Billing

`[defaults] billing = "subscription"` (the default) keeps claude runs on the Claude subscription login and off API credits:

- `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL` and `CLAUDE_CODE_USE_BEDROCK`/`_VERTEX`/`_FOUNDRY` are removed from every claude child process.
- `OPENAI_API_KEY`, `OPENAI_BASE_URL` and `CODEX_API_KEY` are removed from every codex child process, phases and eval trials alike.
- `--bare`, which forces API-key auth, is dropped from the command.
- `ns run` refuses to start (exit 2) when `~/.claude/.credentials.json` is missing and `CLAUDE_CODE_OAUTH_TOKEN` is unset.

`billing = "api"` turns all three off. The user config's `[eval] billing` does the same for `ns eval`.

`ns ask` removes the same variables from the claude or codex command it runs. It reads `billing` from the factory of the repo holding `--cwd` (else the current directory), and treats no repo or no factory file as `subscription`. Under `api` it passes the environment through.

When the plan's usage limit is hit, `claude -p` ends with an error `result` event whose text mentions a usage limit, a rate limit, or "limit reached". That is not a phase failure: no attempt is used, the unit isn't stuck, and `ns run` ends with `paused` (exit 4), `reset_at` set when the message carries a reset time (`...|<unix seconds>`, or `resets 3am` in local time). Re-running resumes where it stopped.

## ns watch

See [ns watch](factory/watch.md): [Parallel units](factory/watch.md#parallel-units), [Triage pass](factory/watch.md#triage-pass), [Main's CI](factory/watch.md#mains-ci), [Split parents](factory/watch.md#split-parents), [Units a night didn't finish](factory/watch.md#units-a-night-didnt-finish).

## Unit lifecycle

See [Unit lifecycle](factory/unit-lifecycle.md): [Gates at a glance](factory/unit-lifecycle.md#gates-at-a-glance), every check between the phases on one page, and [Cleanup](factory/unit-lifecycle.md#cleanup).

## Quality

See [Quality](factory/quality.md): [Where the history lives](factory/quality.md#where-the-history-lives).

## Trust

Phases run unattended with permissions bypassed, and on a public repo anyone can open an issue or comment on one. On GitHub the team is the authors whose association is `OWNER`, `MEMBER` or `COLLABORATOR`. On GitLab, which has no association, it is the project's members, inherited group members included, at Developer access (30) or above: Developer, Maintainer and Owner. Reporter, Planner, Guest and non-members are outside it.

- **Only team-authored issues are queued.** `ns watch` skips any other issue with reason `author outside the team`. A missing association, or an author not among the members, counts as outside the team.
- **Only team comments are instructions.** Phases quote other text as data and never act on it. The rule lives in `skills/ns-contract/SKILL.md` under "Untrusted issue content", and the skills that read issues point there.

What remains: the team's own text is trusted in full, so a team member who pastes untrusted text into an issue or comment passes it through as instructions. `ns run --issue <n>` started by hand runs any issue, whoever wrote it, and only the skills' rule guards its comments.

## Hook guard

Every claude phase runs `ns guard-hooks` before each Bash, Edit, Write, MultiEdit and NotebookEdit call ([Run](factory/run.md), "Phases don't skip the repo's git hooks"). It reads a Bash command as shell words, and a word built by a shell expansion is one it may not be able to read:

- **What it resolves.** `NS_UNIT` and `NS_WORKTREE`, from its own environment, which `ns run` gives the phase; and a variable the command assigns a literal value to before it uses it (`NS_UNIT=142-uart-timeout; cd .ns/$NS_UNIT`). A value is written into the word, and the word is judged as if typed out.
- **What stands in for the rest.** Every other expansion stays unread. To learn whether a denial came only from such a word, the guard judges the command again with each unread expansion replaced by `unread`, a name no protected path or flag has.
- **What the denial says.** When the stand-in lets the command through, only an unreadable word caused the denial. The reason starts `ns: denied:` and tells the phase to write the word out literally, or write the file with the Write tool, and carry on: the denial is no reason to stop. Any other denial starts `ns: blocked:` and names the rule it enforces.

Claude Code's own permission rules read paths the same way, without the guard's resolution: it can't resolve a glob in a `cd` (`cd .ns/142*/`), so it can't rule out a `secrets` directory behind it, and `Read(//**/secrets/**)` denies the call. So a phase names its unit directory exactly, `.ns/$NS_UNIT/`, and never with a glob (`skills/ns-contract/SKILL.md`, "Units of work and artifacts").

## Starting a night

See [Starting a night](factory/starting-a-night.md): [Before the first night](factory/starting-a-night.md#before-the-first-night), [Install the timer](factory/starting-a-night.md#install-the-timer), [When the watched repo is nightshift](factory/starting-a-night.md#when-the-watched-repo-is-nightshift), [Stopping a night](factory/starting-a-night.md#stopping-a-night), [Supervising a night](factory/starting-a-night.md#supervising-a-night), [In the morning](factory/starting-a-night.md#in-the-morning), [Resuming a unit](factory/starting-a-night.md#resuming-a-unit).
