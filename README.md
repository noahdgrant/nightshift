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

1. Run `ns init` and commit what it leaves: `AGENTS.md`, which points to the coding standards that hold in any language and that `ns` writes to `.nightshift/coding-standards.md` (when the repository has neither `AGENTS.md` nor `CLAUDE.md`, `ns` writes both, `CLAUDE.md` holding `@AGENTS.md`; an existing one is only given the pointer); `.nightshift/coding-standards.md` itself; `.nightshift/nightshift.toml`, which pins the repository to this version of `ns`, `.claude/settings.json`, whose hook keeps the skills current in a Claude Code session, and `.github/workflows/closes-unit.yml`, the check that fails a change request that closes no unit (an existing workflow there is left alone), and `.nightshift/check`, the command `ns` runs as the repository's checks, and four short pages for a person to fill in, `docs/agents/stack.md` (read by `ns-triage` and `ns-design-doc`), `docs/agents/how-to-run.md` (read by `ns-verify`) and `docs/agents/labels.md` (read by `ns-triage`) and `docs/agents/review.md` (read by `ns-review`: where review looks hardest and what it leaves out); an existing page is left alone. `ns init` looks for that command (`make check` when a `Makefile` has a `check` target, else the usual checks for `Cargo.toml`, a `package.json` with a `test` script, `go.mod` or `pyproject.toml`), proposes it, and asks you to accept or edit it; when it finds nothing it asks with no proposal. `--check '<command>'` gives the command and skips the question, and an existing `.nightshift/check` is left alone and not asked about. With no terminal, no `--check` and nothing found, `ns init` fails rather than leave the repository with no checks. It also creates the labels `ns` uses on the forge, if they are missing, and sets the description of each one it owns when that differs from this version's (`ns doctor` fails on a label whose description differs, and says to run `ns init`; labels outside its set are left alone): the `status:` labels it sets, the `type:` labels (the Conventional Commit types) and the `priority:` labels (`critical`, `high`, `medium`, `low`) that order the queue. In a session, type `/ns-grilling` or any other skill.
2. If the command `ns init` wrote to `.nightshift/check` is wrong, replace it with the one that runs the repository's checks (format, lint, tests), such as `scripts/ci-local.sh`, and commit the file.
3. Create the `area:` labels for the parts of your project. `ns triage` only uses labels the repository has, and only you know the areas.

A unit's type is set by the repository's kind. Where the owner is an organization with native issue types, `ns triage` sets Feature for `feat` and Bug for `fix` and adds no `type:` label for them (`ns init` does not create `type:feat` or `type:fix` there); every other type is set as Task and also keeps its `type:` label, which records the exact type. `ns init` never creates or changes a native type, since that is an organization setting. If the organization has renamed or removed Feature, Bug or Task, or the repository is a personal one with no native types, the `type:` labels carry the type, as before.

A file can fence lines a person must review with a start marker and an end marker, each a line of a comment in any syntax. The start marker may give a reason after a colon. Regions do not nest. `ns check` runs before the repository's checks and fails, naming the file and line, on a start with no end, an end with no start, or a start inside another region. It reads the text files git tracks; a file with no markers is fine. This is what a region looks like:

```
# ns:human-review start: the retry limit is a business rule
MAX_RETRIES = 3
# ns:human-review end
```

Run `ns doctor` to confirm the setup. It prints a JSON object with one check each for the pin, the harness, the forge login, `.nightshift/check`, the labels, the classic branch protection of the default branch (a branch protected only by rulesets reads as unprotected), the `closes-unit` workflow and the coding standards (the file must equal the pinned version's), says how to fix any that fails, and exits 1 when one does. It changes nothing.

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

A review looks at the change in six review areas: correctness, security, spec (every requirement implemented in full, nothing beyond it), tests, architecture, and docs and comments (the change's comments and docs are true, say why and not what, and the docs it makes stale are updated). `[review] sessions` in `.nightshift/nightshift.toml` sets how many harness sessions share them, one after another. With `3`, the default, the groups are behavior (correctness and security), requirements and tests (spec and tests) and maintainability (architecture, and docs and comments). With `6` each area has its own session, and with `1` one session reviews all six. Each session is shown only its own earlier findings that were not met. The review fails when any session ends `VERDICT: FAIL`, and passes only when all pass. More sessions find more and show which area missed what, and cost more subscription time. Any value but 1, 3 or 6 is an error, and `ns` stops before it runs anything.

```toml
[review]
sessions = 3
```

Each place a headless session starts can use its own model. `[model]` in `.nightshift/nightshift.toml` has the keys `default`, `triage`, `troubleshoot`, `build`, `verify` and `review`. A session uses its own key, then `default`, then the harness's own default model. All of a review's sessions use `review`. `--model <name>` on `ns run`, `ns auto`, `ns triage`, `ns troubleshoot` and `ns eval review` beats every key for that command. A value is the harness's own model name or alias, passed through as it is; `ns` does not check that the harness knows it, and a name the harness rejects fails the session. A key `ns` does not know, or a value that is not a non-empty string, is an error, and `ns` stops before it runs anything. The run log records the model of each move out of `build`, `verify` and `review`, and `ns log` prints it beside each session.

```toml
[model]
default = "sonnet"
build = "opus"
review = "opus"
```

`ns status` shows where each unit stands in the morning.

`ns grades [--weeks <n>]` shows whether a change to the factory moved the grades. It reads `grades.jsonl` in the `ns` state directory and prints two plain-text tables: each rubric's pass rate, with the number of grades in brackets, for each of the last `n` ISO weeks (8 without the flag, the current week included), and the same for each `ns` version that graded in those weeks. A week or version with no grades for a rubric shows `-`. With no grades it prints one line saying so. `--json` prints one JSON line for each rubric and period instead. It changes nothing and starts no session.

`ns improve [--since <days>] [--dry-run]` turns the grades that did not pass in the last `days` days (7 without the flag) into units that fix their cause. A person runs it by hand; `ns auto` never does. When some rubric has failures from at least 3 different units, one read-only harness session groups them by cause and proposes up to 3 units, each with a Conventional Commits title and a brief (Goal, Requirements, Out of scope, Open questions) that cites the units and grades behind it and names the layer to fix: `code`, `check` or `prose`, the highest that removes the whole category. Each unit's description starts, after the disclosure, with a marker `<!-- ns:improve <rubric> <cause> -->`, and a cause whose marker an open unit already carries is not filed again. A filed unit has the `type:`, `priority:` and `area:` labels the repository has and `status:needs-triage`, never `status:ready-for-agent`, so a person looks first. In an ordinary repository a proposal changes only what the repository owns (`docs/agents/*`, `AGENTS.md`, the coding standards, `.nightshift/check`, lint rules, tests and code); a cause that lies in a skill is filed under "For the skill's owner" with no change to the skill. `ns` prints one JSON line for each unit as soon as it is filed, or with `--dry-run` for each it would file, which files nothing; with nothing to propose it prints `{"improve":"nothing to propose"}`. It edits no file and opens no change request.

Every headless session leaves its transcript, what the agent read and ran and not only its final text, as `transcripts/<unit>/<state>-<n>.jsonl` in the repository's `ns` state directory (`.git/ns` with git). It outlives the unit's workspace. `ns log <unit>` shows it; `--state` and `--last` narrow it. A session is typically tens of kilobytes to a few megabytes of JSON, and a unit that goes round `build` many times keeps one for each, so `ns run` and `ns auto` delete transcripts last modified more than `[run] transcript_days` days ago, in `.nightshift/nightshift.toml`. Without the key they keep a week's; `0` keeps them for ever. Deleting `transcripts/` is safe; `ns` only reads it for `ns log`.


Once `ship` has merged a change, the `retrospective` state looks back at the unit and grades it on four default rubrics (`efficiency`, `redundant-tests`, `verbosity` and `followed-the-brief`, the files in `rubrics/`) and on the repository's own. A repository's rubrics are files `docs/agents/rubrics/<name>.md`, read from the unit's workspace: a file named like a default replaces it, any other name adds a rubric. A file starts with front matter holding `name` (the file's name without `.md`) and `pass` (a number from 0 to 1, the lowest score that passes), then a body that says what to judge and gives a scale of labelled values from 0 to 1. A file that is not valid is left out of that retrospective and the run log says why; `ns check` fails on one, naming the file. Each grade is appended as a JSON line to `grades.jsonl` in the `ns` state directory (`.git/ns` with git), next to the run log, and is never pruned. A retrospective that goes wrong is logged as skipped and never sticks the unit. `[retrospective]` in `.nightshift/nightshift.toml` sets `enabled` (default `true`) and `model` (default: the harness's own). `docs/STATES.md` has the rest.

```toml
[retrospective]
enabled = true
model = "opus"
```

`ns eval review` runs the review sessions `[review] sessions` sets on the known cases in `evals/review` and reports whether it gives the right verdict each time, so you can tell whether an edit to the skill helped. `--model` runs the cases with another model, to compare models on the same cases. It makes real harness calls, so it uses subscription time.

`ns eval add review <commit> --unit <number> --as <case> [--expect <PASS|FAIL>]` writes the change a commit made, and the unit it was for, as a new case in `evals/review`. Use it after a defect got past review and was found later: the change that carried it becomes a case, so the next edit to the skill is checked against it. `--expect` is `FAIL` when absent. It writes nothing and fails when the case already exists, the commit is not found or the unit cannot be read. It prints one JSON line naming the directory; read the case and commit it.

## Credits

Adapted from [mattpocock/skills](https://github.com/mattpocock/skills), [cursor/plugins pstack](https://github.com/cursor/plugins/tree/main/pstack) and [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills). See [`NOTICE.md`](NOTICE.md).
