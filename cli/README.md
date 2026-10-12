# ns

The Nightshift CLI. It creates one worktree per unit of work, runs a configured harness headless for a role, drives units through the phases unattended (`ns run`, `ns watch`), lints skills, installs them into harness skill directories, and measures them with evals.

## Install

```sh
cargo install --path cli
```

## Conventions

- Machine output is JSON on stdout. Errors go to stderr, with a correct invocation under them.
- Nothing prompts. Every input is a flag, a positional argument, or stdin.
- Commands are safe to re-run. Destructive commands take `--dry-run`.
- `ns <command> --help` lists options and examples for that command.

| Exit | Meaning |
|---|---|
| 0 | ok |
| 1 | failure (git error, lint errors, install conflicts, harness exited non-zero) |
| 2 | usage error (bad unit id, missing path, unreadable prompt file) |
| 3 | `ns ask`: role not configured, or no config file. `ns eval`: eval harness not configured, or the config doesn't parse |
| 4 | `ns ask`, `ns eval`: harness binary not on PATH |
| 5 | `ns ask --write`: the harness has no `command_write` |

`ns run` has its own codes: 0 done, merged or split, 1 stuck, 2 usage or config error, 3 budget, 4 paused on a usage limit, 5 another `ns run` holds the unit's run lock.

## Commands

### `ns worktree new <unit-id> [--base <branch>] [--repo <path>] [--no-setup]`

Creates `<root>/../<repo-name>.worktrees/<unit-id>` on branch `ns/<unit-id>`, where `<root>` is the main worktree of the repo you run it from. The base defaults to the local branch behind `origin/HEAD`, else the current `HEAD`. It also creates `.ns/<unit-id>/` in the worktree and adds `.ns/` to the common `.git/info/exclude`.

Prints `{"unit","path","branch","artifacts","setup"}`. If the worktree exists, it prints the same JSON. If only the branch exists (after `remove`), it checks the branch out again. Unit ids match `[a-z0-9][a-z0-9-]*`.

When the worktree is newly created, `new` runs the setup commands from `<root>/.nightshift/nightshift.toml`:

```toml
[worktree]
setup = ["git submodule update --init"]
```

Each command runs with `sh -c` in the new worktree, with `NS_UNIT`, `NS_WORKTREE` and `NS_MAIN_ROOT` set. Their output goes to stderr. `setup` in the JSON lists `{"run","exit"}` per command that ran. The first failing command stops the rest and makes `new` exit 1. The worktree stays in place, and `ns worktree setup <unit-id>` retries. An idempotent repeat of `new` runs nothing and prints `"setup": []`, and so does `--no-setup`.

`.nightshift/` is the factory definition root (D15 in `docs/DESIGN.md`). The whole file is parsed against the schema in [docs/FACTORY.md](../docs/FACTORY.md), and unknown tables or keys are rejected (exit 2).

### `ns worktree setup <unit-id> [--repo <path>]`

Re-runs the `[worktree] setup` commands in the unit's existing worktree. Prints `{"unit","path","setup"}` and exits 1 if a command fails. A unit with no worktree is an error that names `ns worktree new`.

### `ns worktree list [--repo <path>]`

Prints a JSON array of worktrees on `ns/*` branches. Each entry has `unit`, `path`, `branch`, `artifacts` and `status`. `status` is `null`, or `{"file","phase","status","updated"}` from the artifact with the newest `updated` in its frontmatter. Ties go to the later phase.

### `ns worktree remove <unit-id> [--dry-run] [--force] [--repo <path>]`

Removes the unit's worktree and keeps its branch. A worktree with uncommitted changes is refused unless `--force` is given. Artifacts under `.ns/` are ignored, so they don't count as changes. Removing a unit that has no worktree prints `"action": "none"` and exits 0.

### `ns ask --role <role> [--prompt-file <f>] [--write] [--cwd <dir>] [--dry-run]`

Runs the harness configured for the role. The prompt is read from `--prompt-file`, or from stdin, and sent to the harness on stdin. The harness's stdout streams through. The default is read-only. `--write` uses the harness's `command_write`, so the harness can edit files and run commands. `--cwd` runs the harness in that directory, such as a unit's worktree. `--dry-run` prints the resolved command, mode, cwd and whether the binary is on PATH, as JSON, without running anything.

The role lookup falls back from the most specific name: `review.security`, then `review`, then `default`.

Under the factory's `billing = "subscription"` (the default, and the mode outside a repo), the harness never sees the API-key variables of claude or codex. See Billing in `docs/FACTORY.md`.

### `ns lint [path] [--human]`

Checks the skills under `path` (default `./skills`). Under `skills/`, a directory is a skill if it has a `SKILL.md` or an `ns-` name, and others are skipped. Under a repo-local root such as `.agents/skills` (the canonicalized root's parent is named `.agents`), every non-hidden directory is a skill, and one without `SKILL.md` reports `missing SKILL.md`. For each skill:

- the frontmatter parses as YAML
- `name` equals the directory name and starts with `ns-` (the prefix is not required under a repo-local root)
- `description` is non-empty and at most 1024 characters
- each `metadata.upstream` entry (a string or a list) matches `owner/repo@<7-40 hex>:path`
- every relative link (`](x)`, `](./x)`, `](../x)`) in any `.md` file of the skill points to an existing path. URLs with a scheme, absolute paths and bare `#anchor` links are skipped, anchors are stripped, and fenced code blocks and inline code spans are skipped.
- each `features/*.md` file (except `features/README.md`) has exactly four H2 headings in order: `Sub-features`, `How to get to it (user POV)`, `Driving it with <tool>`, `Gotchas`. A mismatch reports `features/x.md: H2 headings must be, in order: ...; found: ...`.
- every `.md` file under the skill's `references/` is linked from some `.md` file in the skill. A link to a directory counts as a link to its `README.md`.

Prints `{"ok","skills","errors":[{"skill","file","message"}]}` and exits 1 if there are errors. `--human` prints one line per error.

### `ns install [--source <dir>] [--harness <h>]... [--target <dir>]... [--dry-run]`

Symlinks each `<source>/ns-*` directory (default source `./skills`) into each target. `--harness` picks a harness's skill directory, and may repeat:

| Harness | Directory |
|---|---|
| `claude` | `~/.claude/skills` |
| `codex` | `~/.agents/skills` |
| `agents` | `~/.agents/skills` |
| `cursor` | `~/.cursor/skills` |

The targets are every `--harness` directory plus every `--target` directory, with duplicates removed. With neither flag, the default is `--harness agents --harness claude`. Target directories are created if missing. Each action is one of:

| Action | Meaning |
|---|---|
| `link` | new symlink |
| `unchanged` | already points at the source |
| `relink` | replaced an `ns-*` symlink that pointed at another checkout of the same skill, or at nothing |
| `prune` | removed a dangling `ns-*` symlink into the source, for a skill that was deleted |
| `conflict` | a real directory or an unrelated symlink is in the way. Left alone, and the command exits 1 |

### `ns eval [skill...] [options]`

Implements [docs/EVALS.md](../docs/EVALS.md): trigger evals from `skills/<skill>/evals/triggers.toml` and behaviour cases from `skills/<skill>/evals/cases/<id>/case.toml`, run against fixtures in `evals/fixtures/<name>/`. With no skill named, it takes every skill that has an `evals/` directory. `tdd` and `ns-tdd` both work.

```
ns eval [skill...] [--case <id>]... [--arms with,without | --compare <ref>]
        [--trials N] [--changed-since <ref>] [--budget-usd X] [--max-runs N]
        [--triggers-only | --cases-only] [--dry-run] [--no-cache] [--out <file>]
        [--skills-dir <dir>] [--no-write-results] [--human]
        [--harness <name>] [--model <m>] [--trigger-timeout <secs>]
```

- `--dry-run` prints the plan as JSON: harness command, skills, cases, arms with the skills each installs, trials planned, cached baseline trials, the resolved fixture env, skipped items with reasons, and estimated runs (`min`, `max`, capped by `max_runs`). It never starts a harness and writes nothing.
- Fixtures are read from `<skills-dir>/../evals/fixtures`. A case whose `fixture` resolves outside that directory stops the command (exit 1). A missing fixture, a missing capability, or a missing skill skips the case with a reason.
- `--case` selects cases and skips trigger evals. `--changed-since <ref>` keeps only skills with a file changed since `ref`, including uncommitted and untracked files.
- `--compare <ref>` runs arms `with` and `old`. `old` installs the case's skills as they were at `ref`, extracted with `git archive`.
- Stdout is one JSON object: runs, cost, budget state, per-skill cases with every trial and check, trigger results, `skipped`, and `results_files`. `--out` writes the same JSON to a file. `--human` adds a table on stderr. Progress lines go to stderr.
- The run exits 0 when it completes, whatever the pass rates. Failing trials are measurements, not errors.

How a trial runs:

1. The fixture is copied to `<tmp>/ns-eval-*/repo` (without `fixture.toml`), `git init`ed and committed. `.ns/` goes into `.git/info/exclude`, as in a real worktree.
2. The case's `files/` overlay is copied in. Other files in the case directory sit beside the repo while `setup.commands` run, so `cp ../brief.md .ns/x/` works. They are removed before the agent starts. The result is committed: this post-setup commit is the base for `diff_scope`, `{changed_tests}`, `fails_on_base` and the judge's diff.
3. `$HOME` is a throwaway directory holding the arm's skills in `.claude/skills/` and `.agents/skills/`, plus symlinks to the harness's `carry` files from your real home. `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `XDG_*_HOME` and `NS_CONFIG` are unset.
4. The harness runs with the prompt on stdin, cwd in the scratch repo, the fixture `[env]` and capability `path_prepend` applied, in its own process group. The group is killed at `timeout_minutes` (default 20). `ns eval` refuses to start a harness anywhere but a scratch dir under the system temp dir.
5. Checks run in the scratch repo with the same environment. The judge calls `ns ask --role eval.judge --cwd <scratch repo>` with the rubric, the case prompt and the diff, and reads `pass` or `fail` from the first line. The scratch dir is then removed.

Trigger evals run each prompt once, in an empty scratch dir with only the skill installed, and are killed after `--trigger-timeout` seconds (default 180). `claude-stream-json` counts a trigger as loaded on a `Skill` tool call naming the skill, or a `Read` of `<skill>/SKILL.md`. `codex-jsonl` counts a shell command naming `<skill>/SKILL.md`. A harness with `output = "none"` reports no metrics and triggers as `unsupported`. User-invoked skills (`disable-model-invocation: true`) have no trigger evals.

Run control:

- **Baseline cache.** `without` trials are stored under `<transcripts>/cache/<key>.json`, keyed by the case directory's content, the fixture's content, harness name, model, and the extra skills' content. A hit is reused, and new trials are appended. `--no-cache` reruns and replaces it.
- **Adaptive trials.** Each arm starts at `trials` and gains one trial per round, up to `max_trials`, while any arm has mixed outcomes.
- **Budget.** No trial starts once the reported cost reaches `budget_usd` or the run count reaches `max_runs`. Whatever didn't run is reported as `budget`. Cached trials cost nothing.

Results: for every skill where something ran, `skills/<skill>/evals/results/<YYYY-MM-DD>-<short-sha>.json` holds the config, the commit and whether the skill dir was dirty, per-case and per-arm metrics (pass rate, median input and output tokens, cost, wall time and turns, outcomes), and skill metrics: `uplift` (mean per-case pass-rate gap against `without`, or `old`), `efficiency` (median tokens and wall time, `with` against the baseline) and trigger precision and recall. No transcripts. `--no-write-results` skips it. Transcripts go to `<transcripts>/runs/<run-id>/<skill>/<case>/<arm>-<n>.jsonl`, next to the harness's stderr.

### `ns factory validate [--factory <dir>]`

Parses `<dir>/nightshift.toml` (default `<main-worktree>/.nightshift/`) and checks gates, billing, merge policy and globs, phase names, harness names (user config or built in) and each `agents/<role>/agent.md` (frontmatter `role` matches the directory, placeholders are known). Prints `{"ok","path","name","phases","errors","warnings"}` with every phase's resolved skill, harness, model, timeout, attempts and prompt source. `warnings` lists keys that load but do nothing, such as the removed `[queue] triage_per_night`; they leave `ok` true. Exits 1 on any error.

### `ns score <unit-id> [--scorer <name>] [--factory <dir>] [--dry-run]`

Grades a finished unit with each scorer in `scorers/` (or the one `--scorer` names): builds a prompt from the rubric and labels, the `*.md` files in `.ns/<unit>/` and the PR outcome from `gh`, runs the scorer's role read-only through the `ns ask` path, and maps the reply's last `label: <value>` line to a score. Appends `{unit,scorer,label,score,pass}` per scorer to `ns/scores.jsonl` in the common git dir, and nothing when any judge names no known label (exit 1). `--dry-run` prints each prompt and writes nothing, but still calls `gh`. See `docs/FACTORY.md`, "Scoring".

### `ns run [<unit-id>] [--issue <n>] [--from <phase>] [--gates stop|auto] [--base <ref>] [--dry-run] [--factory <dir>]`

Implements `ns run` in [docs/FACTORY.md](../docs/FACTORY.md): creates or reuses the unit's worktree, reads `.ns/<unit>/*.md`, picks the next phase from the state table, runs the phase's harness headless in the worktree with the rendered prompt on stdin, and repeats until the unit is done, merged, split, stuck, paused or out of budget. With only `--issue`, the unit id is `<n>-<slug of the issue title>` (read with `gh issue view`).

- **Harness.** Each phase uses the `command_write` of its harness: `[harness.<name>]` from the user config, else the built-in. The built-in claude command for `ns run` is `claude -p --permission-mode bypassPermissions --model {model} --output-format stream-json --verbose`. Headless `acceptEdits` can't run shell commands, and the phases need Bash. Permissions are bypassed, so the agent can run any command as your user; cwd is always the unit's worktree, but nothing else confines it. Run it on a machine where that is acceptable. A configured claude command gets `--output-format stream-json --verbose` added if it lacks them.
- **Files.** Transcripts: `<git-common-dir>/ns/transcripts/<unit>/<phase>-<attempt>.jsonl` (plus `.stderr`). Run log: `<git-common-dir>/ns/runs.jsonl`. Locks: `<git-common-dir>/ns-run-<unit>.lock` (one `ns run` per unit), `<git-common-dir>/ns-watch.lock` (one `ns watch` per repo), `<git-common-dir>/ns-merge.lock` (one merge step's update, CI wait and merge at a time per repo) and `<git-common-dir>/ns-worktree.lock` (one `ns worktree new` creating a worktree at a time per repo), each an OS `flock` whose file names the holder while held.
- **Guards.** After every phase: the default branch on `origin` must not have moved to a commit of this unit, and a PR named in `pr.md` must not be merged. Either stops the run as stuck.
- **Merge.** With `[merge] policy = "auto"`, a passing `pr.md` leads to the merge step: with a gate and `local_queue` on (the default), gate the merge of the PR head with the default branch in a scratch worktree; else update a branch that is behind and gate the PR head; wait for checks to register on the PR head (bounded by `ci_register_timeout`), wait for CI (bounded by `ci_timeout_minutes`), send CI failures and conflicts back to build, leave PRs that change `human_review` files to a human, else `gh pr merge --squash`. Outcome `merged`.
- **Quality records.** When a unit ends `merged`, `done` or `stuck`, `ns run` pushes one record per review attempt to `origin`'s `nightshift/quality` branch. If the push fails, the records wait in `<git-common-dir>/ns/quality-outbox.jsonl` for the next run, and the unit's outcome is unchanged.
- **Cleanup.** After its merge step merges the PR and the unit's records are saved (pushed, or in the outbox), `ns run` removes the unit's worktree and local `ns/<unit>` branch. A worktree with uncommitted changes to tracked files, or a branch holding changes the merged PR head doesn't, stays. `cleanup` in the output says which, and why.
- **Billing.** `[defaults] billing = "subscription"` (the default) removes `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL` and `CLAUDE_CODE_USE_BEDROCK`/`_VERTEX`/`_FOUNDRY` from claude's environment and `OPENAI_API_KEY`, `OPENAI_BASE_URL` and `CODEX_API_KEY` from codex's, drops `--bare`, and refuses to start (exit 2) without `~/.claude/.credentials.json` or `CLAUDE_CODE_OAUTH_TOKEN`. A usage-limit error from claude ends the run as `paused` (exit 4) without using an attempt. `budget_usd` is a soft cap on the reported cost, unset by default on a subscription.
- `--dry-run` prints the next decision, the rendered prompt and the command. It creates no worktree, takes no lock, and calls no harness.

### `ns watch [--once] [--until HH:MM] [--max-units N] [--parallel N] [--dry-run] [--factory <dir>]`

Lists open issues labelled `[queue] ready_label` with `gh`, drops issues whose first line is `Blocked by: #a, #b` with any blocker open, and issues an open PR closes (unless the PR is from the issue's own unit branch, which resumes at the merge step), then sorts by the first matching `[queue] order` label and the issue number. For each unit it swaps the ready label for the in-progress label, fetches `origin`, and starts `ns run --issue <n> --base origin/<default>` as a child process on a snapshot of the config and factory definition taken at start, up to `--parallel` (or `[limits] parallel`, default 1) at once; a usage limit in one unit pauses the others at their next phase, and the budget counts every unit's spend (`docs/FACTORY.md`, "Parallel units"). Afterwards: `merged` removes the in-progress label (and `human_merge_label` from the PR), `done` sets `done_label` and labels the unit's PR, if it has one, `human_merge_label` (default `needs-human-merge`, created in red when missing) with a comment saying why it needs a human merge, so `gh pr list --label needs-human-merge` is the merge queue, `split` (triage split the issue into child issues) sets `split_label`, `stuck` (or a `done` that needs a human merge, whose PR is labelled too) sets `stuck_label` and comments the reason with an AI disclaimer, `budget` (also a runner lock or merge lock wait cut short by `--until`, or a budget found spent once the locks are held) returns the issue to the ready label and stops, and `paused` sleeps until the usage limit resets and resumes the same unit, or returns the issue and stops if the reset is past `--until`. At start it returns open in-progress issues that no live `ns run` holds to the ready label, keeping their worktrees so the units resume (`requeued` in the output). At start and between units it removes the worktrees of units a human merged since, as `ns clean` does (`cleaned` in the output, `clean` under `--dry-run`). On SIGINT or SIGTERM it starts nothing new, passes the signal to each unit's `ns run`, which ends its running phase's process group (SIGTERM, then SIGKILL after 5 s), returns every unit's issue to the ready label, prints the summary with `stopped: "SIGINT"` or `"SIGTERM"`, and exits 130 or 143. A second `ns watch` on the repo exits 5. `--dry-run` prints the ordered queue, the skip reasons, the in-progress issues it would return (`requeue`), and the label it would put on a PR that needs a human merge (`human_merge_label`). `gh` and every phase get `GH_TOKEN` from `[forge.github]` in the config, or from the environment when it is already set (see [`[forge]`](#forge)). `--until` is local time, and the `until` and `reset_at` it prints are local with their offset (`2026-10-09T06:30:00-04:00`). `NS_NOW` (unix seconds) pins the clock for tests.

### `ns clean [--dry-run]`

Removes every unit worktree, and its local `ns/<unit>` branch, whose issue is closed and whose PR (from `pr.md`) merged, closes the issue and comes from `ns/<unit>`. It saves the units' quality records first; records left in the outbox count as saved. It never removes a worktree with uncommitted changes to tracked files, one whose HEAD left `ns/<unit>`, one whose branch holds changes the merged PR head doesn't (whitespace counts), or a unit whose run lock a live `ns run` holds. Prints `{ok, command, dry_run, status, removed, kept, records}`, with a reason for every kept unit; `status` is `cleaned`, `planned` or `nothing-to-clean`. `--dry-run` prints `planned` and removes or records nothing, though it reads `gh` and may fetch a PR head. Exits 1 when a removal failed. Details in `docs/FACTORY.md` (Cleanup).

### `ns log [--unit <id>] [--since <time>] [-f] [--json] [--phase-output]`

Prints `<git-common-dir>/ns/runs.jsonl`, the events `ns run` and `ns watch` append, one line per event: time, event, issue, phase, attempt, status, duration, outcome, PR link and reason, leaving out what the event lacks. The PR link is built from `origin`'s URL. `-f` prints the log so far, then polls it every 0.5 s for new lines, like `tail -f`. `--unit` keeps one unit's events, and `--since` (UTC, inclusive) drops older and undated ones. `--json` prints one object per event holding only `ts`, `event`, `unit`, `issue`, `phase`, `attempt`, `status`, `wall_s`, `outcome`, `pr`, `pr_url` and `reason`, so prompts, transcript paths and errors in the log never reach the output. Lines that are not JSON are skipped and counted on stderr.

`--phase-output` prints, instead of the run log, the newest phase transcript `ns run` wrote under `<git-common-dir>/ns/transcripts/<unit>/` (of the unit the run log names last without `--unit`; its path goes to stderr): the last 20 assistant messages and tool calls, one line each (`assistant: <text>`, `tool: <name> <input JSON>`), text cut at 200 characters as `ns transcripts read --compact` cuts it. Known credential shapes are replaced by `[redacted]` first: tokens with a known prefix (`ghp_`, `glpat-`, `sk-`, `AKIA`, ...), JWTs, bearer and basic credentials, URL passwords, `--password X`-style flags, `curl -u user:pass`, private key blocks, and the rest of the line after any `*TOKEN*`, `*SECRET*`, `*PASSWORD*`, `*API_KEY*`-like assignment (a JSON key's value too). Other secrets can still show. `--json` prints `{"kind":"assistant","text"}` or `{"kind":"tool","name","input"}` per entry. `-f` then follows that transcript and moves on to the next phase's. It does not take `--since`, and exits 1 when the unit has no transcript yet.

### `ns status [--since <time>] [--json]`

Summarises the night in three parts. Running: each unit a live `ns run` holds (its run lock), with its last logged phase. Ended: each other unit by the outcome of its last `end` event at or after `--since` (UTC, inclusive; default 24 hours ago): merged, PR for a human (`done`), stuck (`stuck` or `budget`), paused, split, and other (anything else, such as `interrupted`, shown only when present). Next: the ready issues `ns watch` would take, in its order, from the same queue reader; when the forge can't be read it prints `queue unavailable` and gives the reason on stderr. Entries hold only the fields `ns log` prints. `--json` prints one object, `{since, running, ended: {merged, pr_for_human, stuck, paused, split, other}, next}`, with `next` null when the queue is unavailable.

### `ns quality [--since <date>] [--json]`, `ns quality import <dir> [--dry-run]`

Reads the quality records on `origin`'s `nightshift/quality` branch (fetched first) and in the outbox. For a unit with no record, it reads the review artifacts in its linked worktree (`.ns/<unit>/review.md`, `review/cycle-*.md`, `history/review*`). It adds the run log and prints JSON: first-pass yield, first-pass Critical and Important findings per 100 changed lines (overall and by axis), cycles to clean, leftovers, escapes blamed to the commit and PR that introduced the line, and a trend by local day. `--since` keeps units whose newest review artifact was updated on or after that local date. `gaps` counts artifacts it couldn't parse and findings missing a field. It changes nothing but the remote-tracking ref. `ns run` writes a unit's records when it ends, and `ns quality import` writes them for archived worktrees. The metrics and their targets are in `docs/FACTORY.md` (Quality).

### `ns transcripts list [--since <date>] [--project <path>] [--new] [--mark]`

Lists Claude Code sessions from `$CLAUDE_CONFIG_DIR/projects` (default `~/.claude/projects`) as `{"index","marked","sessions"}`, oldest first. Each session has `id`, `source`, `date` (its first event's timestamp), `project` (its working directory), `size`, `mtime` and `path`. `--since 2026-10-01` keeps sessions that started on or after that UTC date. `--project <path>` keeps sessions run in that directory (trailing slashes and symlinks are normalised; a session with no recorded working directory is matched by its Claude directory name). `--new` keeps sessions whose size or mtime differ from the index at `$XDG_DATA_HOME/nightshift/transcripts-index.json` (default `~/.local/share/nightshift/`). `--mark` records the listed sessions there as processed.

### `ns transcripts read <id> [--compact]`

Prints one session (an id or a unique prefix) as `{"id","source","date","project","path","entries"}`. Entries are user prompts, assistant text, tool calls with their JSON input, tool output, and errors, in order. Thinking and injected meta messages are left out. Text over 2000 characters ends in `…[truncated N chars]`. `--compact` cuts at 200, tool output included. Exits 1 if no session matches, 2 if the prefix matches several. Entries are verbatim and can contain secrets (tokens in tool output, pasted credentials), so review before sharing or committing them.

Each harness's store is a `Reader` in `cli/src/transcripts.rs`. Only Claude Code has one so far. `--project` takes the path itself and the reader decides what matches. The same id under two project directories is two sessions: the index keys on source and store path, and `read` exits 2 with both paths.

### `ns upstream status [--skills-dir <dir>] [--human]`, `ns upstream diff <skill> [--human]`

Compare adapted skills with the upstream each records in `metadata.upstream` (`owner/repo@sha:path`, one or a list). Each repo is fetched with `git` from `https://github.com/owner/repo` into a bare cache at `<git-common-dir>/ns/upstream/owner/repo`: `--depth=1` the first time, plain fetches after that, and `--deepen` (64 commits, doubling) until the cache holds the recorded sha, so a short sha needs no fetch by id. A repo is fetched once per invocation. git's `url.<base>.insteadOf` redirects the fetch, to a mirror or, in the tests, to a local bare repo.

`status` prints `{"ok","changed","skills":[{"skill","upstreams":[{"upstream","head","changed"}]}]}` for every skill with an upstream; `changed` says whether the path differs between the recorded sha and upstream HEAD. An entry that can't be checked (bad format, fetch failed, sha not in HEAD's history) has `error` instead, and the command exits 1. `diff` prints `{"skill","upstreams":[{"upstream","head","diff"}]}`, the path's diff from the recorded sha to HEAD; `--human` prints each diff under `=== <upstream> -> <head>`. An unknown skill, or one with no upstream, exits 2.

### `ns doctor`

Prints the config path, whether it exists and parses, and each configured role with its resolved harness, model, read-only and write commands, and whether the binary is on PATH. `distinct_harnesses` lists the providers in use, so a skill can tell whether two roles get a cross-provider check. `harnesses` reports which of `claude`, `codex`, `cursor-agent`, `gemini` and `opencode` are on PATH. `forge` reports, per forge, whether it is `configured`, whether a token resolved (`token_resolved`), the `account` the forge's API returns for that token (`gh api user`, `glab api user`), and the `table` it came from. For the forge `origin` is on it also reports the `repo` from `origin`'s URL, whether the token can read it (`repo_readable`; false is a problem), and for an SSH origin the account `ssh -T` greets (`ssh: {dest, account}`). Fields that don't apply are null. The token itself is never printed. `warnings` lists what doesn't stop a night: an SSH account that differs from the token's, an SSH host that greets no account, and a set `GH_TOKEN` or `GITLAB_TOKEN` that overrides the config. `claude_phases` reports that `ns run` phases and `ns eval` trials run claude with auto-memory off (`auto_memory: false`) and the variable that does it (`env`). `skill_dirs` reports, per harness (`claude`, `codex`, `agents`, `cursor`), the `path` that `ns install --harness` links into and `ns_skills`, how many `ns-*` skills it holds (0 when the directory is missing). `problems` lists anything that would make `ns ask` fail, and a forge token that doesn't resolve.

## Config

Path: `$NS_CONFIG`, else `$XDG_CONFIG_HOME/nightshift/config.toml`, else `~/.config/nightshift/config.toml`.

```toml
# Map roles to a harness and an optional model.
[roles.default]
harness = "claude"

[roles.review]
harness = "claude"
model = "opus"

[roles."review.security"]
harness = "codex"
model = "gpt-5-codex"

# Harnesses: argv for read-only runs, and optionally for --write runs.
[harness.codex]
command = ["codex", "exec", "--sandbox", "read-only", "-m", "{model}"]
command_write = ["codex", "exec", "--sandbox", "workspace-write", "-m", "{model}"]

[harness.gemini]
command = ["gemini", "-m", "{model}"]
```

- `{model}` is replaced with the role's `model`. If the role has no model, an argument that is exactly `{model}` is dropped, along with the flag before it. So `--model {model}` disappears and the harness uses its own default.
- `claude` and `codex` are built in. Define `[harness.<name>]` to override a built-in or to add another harness:

  | Harness | `command` (read-only) | `command_write` |
  |---|---|---|
  | claude | `claude -p --permission-mode plan --model {model}` | `claude -p --permission-mode acceptEdits --model {model}` |
  | codex | `codex exec --sandbox read-only -m {model}` | `codex exec --sandbox workspace-write -m {model}` |

- `acceptEdits` lets Claude edit files without asking, but shell commands still need approval, and a headless run cannot give it. A verify or build worker that has to run tests needs more. Either add `--allowedTools` for the commands it needs, or use `--dangerously-skip-permissions`. That flag lets the agent run any command as your user, so use it only in a disposable worktree or a sandbox.
- Unknown keys are rejected, so a typo fails loudly. Run `ns doctor` after editing.

### `[eval]`

Every key is optional. The defaults are shown:

```toml
[eval]
harness = "claude"            # a table under [eval.harnesses], or the built-in claude
# model = "haiku"             # unset: `--model {model}` is dropped
trials = 2                    # starting trials per arm
max_trials = 5                # adaptive ceiling
budget_usd = 5.0              # per invocation
max_runs = 40
transcripts = "~/.local/share/nightshift/evals"
billing = "subscription"      # or "api"; subscription strips API-key variables and --bare from claude

# Built in; define it to override.
[eval.harnesses.claude]
command = ["claude", "-p", "--model", "{model}", "--output-format", "stream-json", "--verbose", "--permission-mode", "bypassPermissions"]
carry = [".claude/.credentials.json", ".claude.json"]
output = "claude-stream-json"   # or "codex-jsonl" or "none"; the default for a new harness is "none"

[eval.harnesses.codex]
command = ["codex", "exec", "--json", "-m", "{model}", "--sandbox", "danger-full-access", "--skip-git-repo-check"]
carry = [".codex/auth.json"]
output = "codex-jsonl"

[eval.capability.zephyr]      # configured when the table exists
base = "~/zephyrproject/zephyr"
sdk = "~/zephyr-sdk-0.17.0"
path_prepend = ["~/zephyrproject/.venv/bin"]
```

### `[forge]`

Where `ns run` and `ns watch` get forge tokens, so a night doesn't need a manual `export GH_TOKEN=...`:

```toml
[forge.github]
token_command = "gh auth token --user <account>"   # a shell command; its trimmed stdout is the token

[forge.gitlab]
token_env = "MY_GITLAB_TOKEN"                      # or: read this variable
host = "gitlab.example.com"                        # optional; exported as GITLAB_HOST (GH_HOST for github)
```

- Set exactly one of `token_command` and `token_env` per forge.
- At start, `ns run` and `ns watch` resolve each token once and export it as `GH_TOKEN` or `GITLAB_TOKEN`. Every child (`gh`, `git`, each phase's harness) inherits it.
- A variable that is already set and non-empty wins, and no command runs. `ns run`, `ns watch` and `ns doctor` warn when it overrides a configured forge: `GH_TOKEN from the environment overrides the config's choice`. `host` likewise doesn't override a set `GH_HOST` or `GITLAB_HOST`.

An operator with more than one account picks the token per owner or per repo, in the same file:

```toml
[forge.github]                                  # the default
token_command = "gh auth token --user personal-account"

[forge.github.owners."example-org"]             # every repo under an owner
token_command = "gh auth token --user work-account"

[forge.github.repos."example-org/backend"]      # one repo
token_env = "BACKEND_TOKEN"
```

- The owner and repo come from `origin`'s URL: `https://github.com/example-org/backend.git`, `git@github.com:example-org/backend.git`, or an SSH host alias such as `git@github-work:example-org/backend.git`. A local path or `file://` URL names no repo, so the default table applies.
- The most specific table wins: the repo's, then its owner's, then `[forge.github]`. Keys match without regard to case.
- Each override takes `token_command` or `token_env`, exactly one, and no other key. An override needs the default table too, since that's where `host` goes.
- GitLab has the same tables, `[forge.gitlab.owners."<group>"]` and `[forge.gitlab.repos."<group>/<project>"]`. A nested group matches on its full path: `owners."team/sub"` covers `team/sub/project` but not `team/project`. The overrides belong to the one `[forge.gitlab]` table, so with `host` set they are that host's: an `origin` on another host (an SSH alias counts as another host) gets the default table, and `ns run`, `ns watch` and `ns doctor` warn when that drops a table that would have matched (`origin host gitlab-work is not gitlab.example.com: ignoring ...`). The host is `GH_HOST` or `GITLAB_HOST` when set, else `host`. If the origin is an SSH alias for that host, put the token in the default table or drop `host`.
- Overrides apply to the forge `origin` is on (`[queue] source`, else its host, as for `ns watch`). The other forge gets its default table.
- After exporting the token, `ns run` and `ns watch` check that it can read the repo (`gh api repos/<owner>/<repo>`, or `glab api projects/<path>`), but only for a forge with a `[forge.*]` table, and not on `--dry-run`. A forge with no table behaves as before. When the check fails they exit 2 before any phase, naming the account the token belongs to (else where the token came from) and the table, and say to check `ns doctor` if the account is wrong. The check and the account lookup each give up after 15 seconds.
- `NS_FORGE_CHECKED` is set by `ns run` and `ns watch` to `<forge>:<owner>/<repo>` once they have checked the repo, so a unit `ns watch` starts doesn't check again. `NS_FORGE_EXPORTED` is set to the same value whenever they settle the token, checked or not (a dry run, a local-path origin), so a unit doesn't warn that `GH_TOKEN` overrides the config. Only a value equal to the current forge and repo counts; leave both unset, and don't export them in a shell.
- Keep these tables in the user config, never in a committed file, so account names stay out of repos.
- `NS_CONFIG` still selects a whole other config file. That works as an escape hatch, but the overrides are the recommended way: they apply in every shell and in the systemd unit without setting a variable.
- A failing command, empty output or an unset `token_env` stops with exit 2. The error names the forge and the command, never the token or the command's output.
- The token is never written to `runs.jsonl`, transcripts, errors or `ns doctor` output.

### `[runners]`

```toml
[runners]
lock_dir = "~/.local/state/nightshift/locks"   # unset: <git-common-dir>/ns/locks
```

Where `ns run` keeps the locks a factory's runners name. Point every repo that shares a bench or a workspace at the same directory, so their phases wait for each other. See `docs/FACTORY.md`, "Runners".

Harness tables live at `[eval.harnesses.<name>]`, not `[eval.harness.<name>]` as `docs/EVALS.md` shows. TOML can't hold `eval.harness` as both the string `"claude"` and a table, so the spec's example doesn't parse.

A capability holds arbitrary string keys plus an optional `path_prepend` list. Fixture `[env]` values expand `{capability.<name>.<key>}` and `~`. The `path_prepend` entries of every capability a case or its fixture requires go on the front of `PATH` for the whole trial.

The judge uses the normal role config: map `eval.judge` (or `eval`, or `default`) under `[roles]`.
