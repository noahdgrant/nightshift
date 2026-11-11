<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.svg">
    <img alt="nightshift" src="docs/assets/logo-light.svg" width="360">
  </picture>
</h1>

An opinionated software factory for building robust and reliable applications. The factory takes a piece of work through the entire software development lifecycle (SDLC): triage, define, plan, build, verify, review, and ship.

## How it works

```
 outer loop                     inner loop (the factory)
 issue ─► ns-triage ─┬─────────────────────────► ns-build ─► ns-verify ─► ns-review ─► ns-ship ─► PR
                     ├─ needs-define ─► ns-define ─► ns-plan ─┘
                     └─ needs-repro ──► ns-troubleshoot ──────┘
```

## Install

From a release, on Linux (x86_64):

```bash
gh release download --repo noahdgrant/nightshift --pattern '*.tar.gz' --dir /tmp/ns
tar -xzf /tmp/ns/ns-*.tar.gz -C /tmp/ns
install -m 755 /tmp/ns/ns-*/ns ~/.local/bin/ns
```

From a checkout, to run what you are editing:

```bash
cargo install --path cli --force
```

The skills are inside the binary. A repository is pinned to one version of `ns`; see [`docs/VERSIONING.md`](docs/VERSIONING.md).

## Setup

Log in to Claude with your Claude subscription (`claude /login`), and tell `ns` which GitHub account opens the PRs and edits labels. The name is one of the accounts `gh auth status` lists:

```bash
ns config forge github --account <account>
```

It is kept in `~/.config/nightshift/config.toml`, not in the repository. A `GH_TOKEN` already set wins over it.

Then, in each project you want to run the factory on:

1. Run `ns init` and commit what it leaves: `AGENTS.md`, which points to the coding standards that hold in any language and that `ns` writes to `.nightshift/coding-standards.md` (when the repository has neither `AGENTS.md` nor `CLAUDE.md`, `ns` writes both, `CLAUDE.md` holding `@AGENTS.md`; an existing one is only given the pointer); `.nightshift/coding-standards.md` itself; `.nightshift/nightshift.toml`, which pins the repository to this version of `ns`, `.claude/settings.json`, whose hook keeps the skills current in a Claude Code session, and `.github/workflows/closes-unit.yml`, the check that fails a change request that closes no unit (an existing workflow there is left alone; an old copy that has `labeled` in its types or a `concurrency` block with `cancel-in-progress` blocks merges once its check is required, because GitHub counts each cancelled run as a failure, so remove them), and `.nightshift/check`, the command `ns` runs as the repository's checks, and four short pages for a person to fill in, `docs/agents/stack.md` (read by `ns-triage` and `ns-design-doc`), `docs/agents/how-to-run.md` (read by `ns-verify`) and `docs/agents/labels.md` (read by `ns-triage`) and `docs/agents/review.md` (read by `ns-review`: where review looks hardest and what it leaves out); an existing page is left alone. `ns init` looks for that command (`make check` when a `Makefile` has a `check` target, else the usual checks for `Cargo.toml`, a `package.json` with a `test` script, `go.mod` or `pyproject.toml`), proposes it, and asks you to accept or edit it; when it finds nothing it asks with no proposal. `--check '<command>'` gives the command and skips the question, and an existing `.nightshift/check` is left alone and not asked about. With no terminal, no `--check` and nothing found, `ns init` fails rather than leave the repository with no checks. It also creates the labels `ns` uses on the forge, if they are missing, and sets the description of each one it owns when that differs from this version's (`ns doctor` fails on a label whose description differs, and says to run `ns init`; labels outside its set are left alone): the `status:` labels it sets, the `type:` labels (the Conventional Commit types) and the `priority:` labels (`critical`, `high`, `medium`, `low`) that order the queue. In a session, type `/ns-grilling` or any other skill.
2. If the command `ns init` wrote to `.nightshift/check` is wrong, replace it with the one that runs the repository's checks (format, lint, tests), such as `scripts/ci-local.sh`, and commit the file.
3. Create the `area:` labels for the parts of your project. `ns triage` only uses labels the repository has, and only you know the areas.

A unit's type is set by the repository's kind. Where the owner is an organization with native issue types, `ns triage` sets Feature for `feat` and Bug for `fix` and adds no `type:` label for them (`ns init` does not create `type:feat` or `type:fix` there); every other type is set as Task and also keeps its `type:` label, which records the exact type. `ns init` never creates or changes a native type, since that is an organization setting. If the organization has renamed or removed Feature, Bug or Task, or the repository is a personal one with no native types, the `type:` labels carry the type, as before.

A file can fence lines a person must review with a start marker and an end marker, each a line of a comment in any syntax. The start marker may give a reason after a colon. Regions do not nest. `ns check` runs before the repository's checks and fails, naming the file and line, on a start with no end, an end with no start, or a start inside another region. It reads the text files git tracks; a file with no markers is fine. This is what a region looks like:

```
# ns:human-review start: the retry limit is a business rule
MAX_RETRIES = 3
# ns:human-review end
```

Run `ns doctor` to confirm the setup. It prints a JSON object with one check each for the pin, the harness, the forge login, `.nightshift/check`, the labels, the protection of the default branch (classic branch protection or an active ruleset, which together must require the `Closes a unit` check), the `closes-unit` workflow, the coding standards (the file must equal the pinned version's) and the `runner` (when one is installed, its unit must match `[runner]` in the settings and the running `ns`), says how to fix any that fails, and exits 1 when one does. It changes nothing.

To move a repository to a newer `ns`, install it, then run `ns upgrade <version>` and commit the new pin.

## Run your first unit

Pick a small issue with clear acceptance criteria, such as a typo or a missing test. Its number is the unit.

```bash
ns triage 12     # decides what it needs next and labels it; a clear one becomes status:ready-for-agent
ns run 12        # runs setup, then stops before build so you can look
```

`ns run` makes the unit's workspace beside the repository (`<repo>.workspaces/12`, on the branch `ns/12`), prints each move as a line of JSON, and keeps the unit's `status:` label up to date. It also assigns the unit and its change request to the account it acts as. Each `ns run 12` runs one state and stops before the next, so the first runs `setup` and stops before `build`; run the same command again to go on from where it stopped, through verify, review and ship. Each state leaves its notes in `.ns/12/` in the workspace. When it is done the change request is open and waiting for you to merge it.

Once you trust the loop, `--gates auto` goes through every state without stopping, and `--merge` lets `ns` merge a change whose checks pass:

```bash
ns run 12 --gates auto --merge
```

A unit that is too vague for `ns triage` to route to build goes to `ns define 12`, a session in which you and the agent work out a spec. `ns run 12 --fresh` throws the workspace away and starts over. A unit is blocked while an open issue blocks it, by GitHub's "blocked by" relationship or a `Blocked by: #a, #b` first line in its description: `ns auto` leaves it waiting, and `ns run` on a unit it has not begun prints the blockers and exits non-zero unless given `--ignore-blockers`; a forge that cannot say counts as not blocked.

## Run it overnight

```bash
ns auto --until 06:30
```

It triages the issues nobody has looked at, then takes the `status:ready-for-agent` ones one at a time and runs each unattended, starting no new unit after 06:30. A unit already running finishes. A unit that gets stuck is labeled `status:stuck` and `ns auto` moves on; pick it up with `ns run`. If two units in a row get stuck because the harness kept failing, not because their checks did, or two triage or troubleshoot sessions in a row fail because the harness did, `ns auto` stops and exits non-zero; `--max-failures-in-a-row` changes the count and `0` turns it off. To try it on a couple of issues first, use `ns auto --max-units 2`, or `--dry-run` to see the queue.

`ns auto` takes a lock for the repository: a second `ns auto` there exits at once with a message and changes nothing, and a lock left by a process that died does not block the next run. Every run that gets the lock, by hand or from the runner, ends by writing a summary, `night-YYYY-MM-DD.md` (the date the night started) in the `ns` state directory (`.git/ns` with git), next to the run log. It lists the units merged, those waiting for a human and those stuck, each with its reason, says why the night stopped early when it did, and says so when no unit was ready. A unit whose labels the forge could not give at the end is listed as stuck, never as merged.

### Start it on a schedule

`ns runner install` writes a systemd user service and timer for the repository you run it in and enables the timer; `ns runner remove` disables and deletes them. Both can be run twice. The service is a `Type=oneshot` that runs `ns auto --until <until>` from the repository's main checkout, with the absolute path of the `ns` that installed it, and its output goes to journald. After `ns auto` it runs `ns improve` once. `ns runner install` prints where the summary files go. The times are in `.nightshift/nightshift.toml`, as a time of day; a value that is not one is an error and nothing is written:

```toml
[runner]
start = "22:00"
until = "06:00"
```

Both keys are optional and those are the defaults, a first guess. Only Linux with systemd is supported.

With `--merge`, `ns run` and `ns auto` merge a change request once its checks pass, unless it touches a file matching `[merge] human_review` in `.nightshift/nightshift.toml`. Then it is left ready to merge, labeled `needs-human-merge`, and the unit is `status:waiting-for-human`. Without the key every change is held for a person; setting it replaces that, and an empty list lets every change merge.

A file that matches `human_review` is held whole. Besides that, before it merges, `ns` compares the change with its merge base and holds it for a person when a changed line falls inside a human-review region (the lines from a start marker to its end marker, both included), in either version of the file, or when a marker line is added, removed or edited. A marker `ns` cannot read, in either version of a changed file, holds it too. A held change request is handled as one that matches `human_review`: it is left ready to merge, labeled `needs-human-merge`, and the unit is `status:waiting-for-human`. The message names the file, the lines of the region and its reason.

```toml
[merge]
human_review = [".github/**", ".nightshift/**", "skills/**"]
```

Before it pushes, `ns` rebases the unit's branch onto the base branch when that has moved, and runs the checks on the result (the human-review markers first, then `.nightshift/check`). A conflict or a failing check sends the unit back to `build`; `verify` and `review` do not run again for a clean rebase. A unit that is going to merge (`--merge`, and no file in the change matches `human_review` or changes a human-review region) takes a lock on the repository before it fetches the base branch and holds it until it is merged or leaves `ship`, so units merge one at a time, each rebased onto a base branch that holds the one before. It waits for the lock without a limit; the wait for the change request's checks under it is bounded by the timeout for checks. It fetches the base branch again before the merge, and if a merge from outside `ns` moved it, the unit goes round again; the merge names the head commit the checks passed. `[merge] queue` turns all of this off (default `true`), for a repository with GitHub's own merge queue or one that does not care, and `[merge] max_base_moves` is how many times the base branch may move, by merges from outside `ns`, while one change request is merged before the unit is stuck (default 3, a whole number of at least 1). A value of the wrong type or range is an error, and `ns` stops before it runs anything.

```toml
[merge]
queue = true
max_base_moves = 3
```

How many times a state may run before its unit is stuck is `[run] max_attempts` in `.nightshift/nightshift.toml`. `--max-attempts` on `ns run` or `ns auto` wins over it, and a repository that sets neither gets the built-in default, which `ns run --help` states. It must be a whole number of at least 1, or `ns` stops before it runs anything.

```toml
[run]
max_attempts = 4
```

How long `ship` waits for the change request's checks to finish is `[ship] ci_timeout_seconds` in `.nightshift/nightshift.toml`, for `ns run` and `ns auto`. `--ci-timeout-seconds` wins over it, and a repository that sets neither waits 1800 seconds (30 minutes). It must be a whole number of seconds above 0, or `ns` stops before it runs anything.

```toml
[ship]
ci_timeout_seconds = 2400
```

A review looks at the change in six review areas: correctness, security, spec (every requirement implemented in full, nothing beyond it), tests, architecture, and docs and comments (the change's comments and docs are true, say why and not what, and the docs it makes stale are updated). `[review] sessions` in `.nightshift/nightshift.toml` sets how many harness sessions share them, one after another. With `3`, the default, the groups are behavior (correctness and security), requirements and tests (spec and tests) and maintainability (architecture, and docs and comments). With `6` each area has its own session, and with `1` one session reviews all six. Each session is shown only its own earlier findings that were not met. The review fails when any session ends `VERDICT: FAIL`, and passes only when all pass. More sessions find more and show which area missed what, and cost more subscription time. Any value but 1, 3 or 6 is an error, and `ns` stops before it runs anything.

```toml
[review]
sessions = 3
```

Each place a headless session starts can use its own model. `[model]` in `.nightshift/nightshift.toml` has the keys `default`, `triage`, `troubleshoot`, `build`, `verify` and `review`. A session uses its own key, then `default`, then the harness's own default model. All of a review's sessions use `review`. `--model <name>` on `ns run`, `ns auto`, `ns triage`, `ns troubleshoot`, `ns eval review` and `ns eval verify` beats every key for that command. A value is the harness's own model name or alias, passed through as it is; `ns` does not check that the harness knows it, and a name the harness rejects fails the session. A key `ns` does not know, or a value that is not a non-empty string, is an error, and `ns` stops before it runs anything. The run log records the model of each move out of `build`, `verify` and `review`, and `ns log` prints it beside each session.

```toml
[model]
default = "sonnet"
build = "opus"
review = "opus"
```

`ns status` shows where each unit stands in the morning.

`ns grades [--weeks <n>]` shows whether a change to the factory moved the grades. It reads `grades.jsonl` in the `ns` state directory and prints plain-text tables: each rubric's pass rate, with the number of grades in brackets, for each of the last `n` ISO weeks (8 without the flag, the current week included), and the same for each `ns` version that graded in those weeks. A week or version with no grades for a rubric shows `-`. When any grade records the model of its unit's last `build` session, a table by that model follows, with the number of grades in brackets and, under it, how many units it left out because their `build` session names no model; a unit that used several models is counted under its `build` model only. With no grades it prints one line saying so. The next table, `Human wait`, has one row for each of those weeks in which a unit that waited for a person was merged: the number of units and the median and 90th-percentile wait in hours (interpolated between the two nearest units). A unit's wait runs from its first `not_merged` line in the run log to the `merged_at` that `ns run` records when it takes over a change request it finds merged, and is counted in the week of `merged_at`; a unit with no `merged_at` is not counted, and with no unit counted the table is left out. A last table, `Cycle time and tokens`, has one row for each of those weeks in which a unit was merged, by the week of `merged_at`: the number of units, the median and 90th-percentile cycle time in hours (from the unit's first move in the run log to `merged_at`, so time `stuck` or waiting for a person is in it) and the median and 90th percentile of tokens, a unit's tokens being input plus output over the sessions its latest grade line records, cache reads and writes left out. A merged unit with no token data is left out of the token figures only, and a line says how many. Under it, one line for each week gives the tokens of units that never merged (no `merged_at`, last move into `stuck`, which a closed change request ends in) in the week of their last move, read from the transcripts still kept (`[run] transcript_days`), so it covers kept transcripts only. The cycle table's last column, `retro share`, is the tokens of the sessions with state `retrospective` as a percentage of the tokens of all the sessions of the week's units, one share for the week and not a median; a unit with no token data is left out of it, and a unit that never merged has no retrospective. A week whose share is above the level is marked `*`, and a line under the table says the level. `[grades] retrospective_share` in `.nightshift/nightshift.toml` sets the level, a percentage from 0 to 100 (25 by default, a first guess); a value outside that is an error and nothing runs. `--json` prints one JSON line for each rubric and period, one with `"kind":"model"` for each row of the model table, one `{"kind":"model_left_out","units":n}` line when some unit was left out of it, one with `"kind":"human_wait"` for each row of that table, one with `"kind":"cycle"` for each row of the cycle table (`median_tokens` and `p90_tokens` are `null` without token data; `no_tokens_units` counts the units left out; `retro_share` is `null` without token data, and `retro_level` and `above_level` carry the level and whether the week is above it) and one with `"kind":"unmerged_tokens"` for each never-merged line, instead. It changes nothing and starts no session.

`ns improve [--since <days>] [--dry-run]` turns the grades that did not pass in the last `days` days (7 without the flag) into units that fix their cause. A person runs it by hand, or the runner does after `ns auto`; `ns auto` itself never does. When some rubric has failures from at least 3 different units, one read-only harness session groups them by cause and proposes up to 3 units, each with a Conventional Commits title and a brief (Goal, Requirements, Out of scope, Open questions) that cites the units and grades behind it and names the layer to fix: `code`, `check` or `prose`, the highest that removes the whole category. Each unit's description starts, after the disclosure, with a marker `<!-- ns:improve <rubric> <cause> -->`, and a cause whose marker an open unit already carries is not filed again. A filed unit has the `type:`, `priority:` and `area:` labels the repository has and `status:needs-triage`, never `status:ready-for-agent`, so a person looks first. In an ordinary repository a proposal changes only what the repository owns (`docs/agents/*`, `AGENTS.md`, the coding standards, `.nightshift/check`, lint rules, tests and code); a cause that lies in a skill is filed under "For the skill's owner" with no change to the skill. `ns` prints one JSON line for each unit as soon as it is filed, or with `--dry-run` for each it would file, which files nothing; with nothing to propose it prints `{"improve":"nothing to propose"}`. It edits no file and opens no change request.

Every headless session leaves its transcript, what the agent read and ran and not only its final text, as `transcripts/<unit>/<state>-<n>.jsonl` in the repository's `ns` state directory (`.git/ns` with git). It outlives the unit's workspace. `ns log <unit>` shows it; `--state` and `--last` narrow it. A session is typically tens of kilobytes to a few megabytes of JSON, and a unit that goes round `build` many times keeps one for each, so `ns run` and `ns auto` delete transcripts last modified more than `[run] transcript_days` days ago, in `.nightshift/nightshift.toml`. Without the key they keep a week's; `0` keeps them for ever. Deleting `transcripts/` is safe; `ns` only reads it for `ns log`.


Once `ship` has merged a change, the `retrospective` state looks back at the unit and grades it on four default rubrics (`efficiency`, `redundant-tests`, `verbosity` and `followed-the-brief`, the files in `rubrics/`) and on the repository's own. A repository's rubrics are files `docs/agents/rubrics/<name>.md`, read from the unit's workspace: a file named like a default replaces it, any other name adds a rubric. A file starts with front matter holding `name` (the file's name without `.md`) and `pass` (a number from 0 to 1, the lowest score that passes), then a body that says what to judge and gives a scale of labelled values from 0 to 1. A file that is not valid is left out of that retrospective and the run log says why; `ns check` fails on one, naming the file. Each grade is appended as a JSON line to `grades.jsonl` in the `ns` state directory (`.git/ns` with git), next to the run log, and is never pruned. A retrospective that goes wrong is logged as skipped and never sticks the unit. `[retrospective]` in `.nightshift/nightshift.toml` sets `enabled` (default `true`) and `model` (default: the harness's own). `docs/STATES.md` has the rest.

```toml
[retrospective]
enabled = true
model = "opus"
```

`ns eval review` runs the review sessions `[review] sessions` sets on the known cases in `evals/review` and reports whether it gives the right verdict each time, so you can tell whether an edit to the skill helped. `--model` runs the cases with another model, to compare models on the same cases. It makes real harness calls, so it uses subscription time.

`ns eval verify` does the same for the verify skill on the cases in `evals/verify`, with the same flags and report; `--model` beats the `verify` key of `[model]`. Each case is a small repository in `repo/`, which each run copies into a new temporary directory and makes a repository with one commit, and the run is scored on its own `VERDICT:` line (the second verification after an INCONCLUSIVE is not replayed). A case holds only commands that are deterministic and need no network. Cases are written by hand (see `evals/verify/README.md`). Each run is a real harness call, so it uses subscription time, and a verification runs commands, so a case costs more than a review session.

`ns eval add review <commit> --unit <number> --as <case> [--expect <PASS|FAIL>]` writes the change a commit made, and the unit it was for, as a new case in `evals/review`. Use it after a defect got past review and was found later: the change that carried it becomes a case, so the next edit to the skill is checked against it. `--expect` is `FAIL` when absent. It writes nothing and fails when the case already exists, the commit is not found or the unit cannot be read. It prints one JSON line naming the directory; read the case and commit it.

`ns eval suggest [--weeks <n>] [--rounds <n>] [--json]` lists the units merged in the last `--weeks` ISO weeks (8 by default) worth keeping as review cases, so you know which to give to `ns eval add review`. A unit is listed when review sent it back to `build` at least `--rounds` times (2 by default), when it was `stuck` before it merged, or when one of its grades did not pass; each line gives the unit's number and title and the reason for each signal, and units with more signals come first. A unit whose title is already on the first line of a case's `unit.md` in `evals/review` is marked as a case. It reads the run log, `grades.jsonl` and the forge (for titles), changes nothing and starts no session. It does not find a unit that carried a defect found by a later fix; add that one by hand with `ns eval add review`.

`ns eval add replay <unit> --as <case>` keeps a past unit as a case in `evals/replay/<case>/`: `unit.md` holds the unit's title and body from the forge as they are now, so a later edit of the issue or an outage of the forge does not change the case; `base` holds the commit its change request was made on, read from the `base` field of the unit's lines in `grades.jsonl`; `unit` holds its number, for the cost estimate. It writes nothing and fails when the case exists, the unit cannot be read, the unit has no grade line, or the commit is not in the repository. It prints one JSON line naming the directory.

`ns eval replay --model <model>... [--runs <n>] [--case <name>] [--harness-timeout-seconds <n>] [--yes] [--max-runs <n>] [--json]` replays each case in `evals/replay` on each model, `--runs` times (3 by default), to choose which model does which state from this repository's own past work. A replay makes a workspace on the case's `base` commit, runs `build` there with the case's brief and the model, runs the repository's checks on the result, and then runs a read-only session that grades the change by the rubrics in force, as the retrospective does. It passes when the checks pass and every rubric passes; a session or the checks running past the timeout is a fail, and so is a replay whose build fails the checks (its grading session is then not run). The workspace is removed afterwards, also after a failure. Only `build` is replayed, and neither the replayed changes nor the sessions' transcripts are kept. For each model it prints the number of replays, the pass rate, the lowest and highest pass rate over the runs (a run is one pass over all cases) and the median tokens of a replay (input plus output, both sessions), or one JSON line with the same figures with `--json`. It exits 1 when the cases cannot be read, a case's commit is not in the repository, or a session hits the usage limit, and otherwise 0 whatever the pass rates are. It picks no model: read the table and set `[model]`. It changes nothing in the repository it is run from and touches nothing on the forge.

A replay costs subscription time: a build session, which is the cost of a whole `build`, and a grading session. Before it starts, `ns eval replay` prints on stderr how many sessions it will run and an estimate of the tokens, taken from the tokens the grade lines record for each case's unit (all of that unit's sessions, so it is an upper bound), and counts the cases with no recorded usage as unknown. It then asks `Start? [y/N]`; `--yes` skips the question, and without a terminal and without `--yes` it refuses to start. `--max-runs <n>` refuses to start when it would run more than n sessions.

## Credits

Adapted from [mattpocock/skills](https://github.com/mattpocock/skills), [cursor/plugins pstack](https://github.com/cursor/plugins/tree/main/pstack) and [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills). See [`NOTICE.md`](NOTICE.md).
