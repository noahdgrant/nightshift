# Evals

How nightshift measures whether a skill helps. Decision D18 in `DESIGN.md` explains why. This file is the spec that `ns eval` implements and that eval cases follow.

## Layout

```
skills/ns-<skill>/evals/
  triggers.toml              does the description fire on the right prompts
  cases/<case-id>/case.toml  one behaviour case
  cases/<case-id>/files/     optional overlay copied onto the fixture before the run
  results/<date>-<sha>.json  committed summaries (raw transcripts stay local)
evals/fixtures/<name>/       starting repos shared by all skills
```

## triggers.toml

```toml
[[trigger]]
prompt = "Fix the off-by-one in reserve(), test first"
expect = true            # the skill should load

[[trigger]]
prompt = "Summarise the README"
expect = false           # the skill should stay out
```

Aim for 10 to 20 per model-invoked skill, roughly half negative. Skip trigger evals for user-invoked skills (`disable-model-invocation: true`).

## case.toml

```toml
description = "Seeded off-by-one in Inventory.reserve; brief asks for a test-first fix"
fixture = "py-inventory"         # evals/fixtures/<name>
skills = ["ns-tdd"]              # skills installed for the "with" arm (default: the owning skill)
extra_skills = ["ns-contract"]   # installed in both arms (dependencies that aren't under test)
requires = []                    # capabilities; the case is skipped when one is missing ("zephyr")
timeout_minutes = 20
prompt = """
Fix the bug described in .ns/7-reserve-overflow/brief.md. gates: auto
"""

[setup]
commands = ["mkdir -p .ns/7-reserve-overflow", "cp ../brief.md .ns/7-reserve-overflow/"]   # `..` holds the case dir's other files during setup only

[[check]]
type = "command"
run = "python -m pytest -q"
expect_exit = 0

[[check]]
type = "fails_on_base"           # the new or changed tests fail on the base commit
run = "python -m pytest -q {changed_tests}"

[[check]]
type = "file_exists"
path = ".ns/7-reserve-overflow/build.md"

[[check]]
type = "frontmatter"
path = ".ns/7-reserve-overflow/build.md"
key = "status"
equals = "pass"

[[check]]
type = "diff_scope"              # every changed path matches one glob
allow = ["src/inventory/**", "tests/**"]

[[check]]
type = "regex"
path = "src/inventory/core.py"
pattern = "def reserve"
present = true

[[check]]
type = "judge"                   # LLM judge via `ns ask --role eval.judge`
rubric = "Did the change fix the root cause rather than special-casing the reported input?"
required = false                 # judges are advisory unless marked required
```

Check rules:

- `{unit}` is not substituted. Write paths out in full.
- `diff_scope` compares the trial's final state with the post-setup commit, counting tracked changes and untracked files that aren't ignored (`git status --porcelain --untracked-files=all`). Paths in `.git/info/exclude` don't count.
- `{changed_tests}` expands to the test files the trial added or changed relative to the base commit.
- A trial **passes** when every required check passes. Checks are required by default, except `judge`.
- `fails_on_base` first runs the command on the trial's final state, which must exit 0. It then copies the changed test files onto a clean checkout of the base commit, runs the command there, and passes when it exits non-zero. The first run rules out a "red" that only means the test runner couldn't start.
- A judge returns `pass` or `fail` on its first line, followed by its reasoning.

## Fixtures

A fixture is a small repo checked in as plain files under `evals/fixtures/<name>/` (no nested `.git`). Each trial copies it to a scratch directory, runs `git init`, commits it as the base, applies the case's `files/` overlay and `setup.commands`, and commits that too. The agent starts from there.

A fixture's `fixture.toml` declares what it needs:

```toml
description = "Zephyr module with ztest suites on native_sim"
requires = ["zephyr"]
[env]
ZEPHYR_BASE = "{capability.zephyr.base}"
```

Capabilities come from the eval config (below). A case or fixture whose capability isn't configured is reported as `skipped`, never as failed.

The fixture's `[env]` applies to everything in a trial: setup commands, the harness run, and every check. `{capability.<name>.<key>}` expands to that capability's value, with `~` expanded. A capability's `path_prepend` list is added to the front of `PATH` for the whole trial (e.g. a venv's `bin/` carrying a toolchain's Python requirements).

## Arms and isolation

Each case runs in arms:

- `with`: `skills` + `extra_skills` installed.
- `without`: `extra_skills` only.
- `--compare <ref>`: replaces `without` with an `old` arm holding the case's skills as they were at a git ref.

Each trial runs the harness with `HOME` pointed at a throwaway directory. The skills for that arm are copied into `$HOME/.claude/skills/` and `$HOME/.agents/skills/`, so nothing the user installed leaks in. The harness's credential files (from the eval config's `carry` list) are symlinked into the throwaway home. The directory is removed after the trial, transcripts excepted.

## Eval config

`ns eval` reads `[eval]` from the user config (`~/.config/nightshift/config.toml`):

```toml
[eval]
harness = "claude"
model = "haiku"
trials = 2                      # starting trials per arm
max_trials = 5                  # adaptive ceiling
budget_usd = 5.0                # per invocation
max_runs = 40
transcripts = "~/.local/share/nightshift/evals"   # raw, never committed

[eval.harnesses.claude]
command = ["claude", "-p", "--model", "{model}", "--output-format", "stream-json", "--verbose", "--permission-mode", "bypassPermissions"]
carry = [".claude/.credentials.json", ".claude.json"]
output = "claude-stream-json"

[eval.capability.zephyr]
base = "~/zephyrproject/zephyr"
sdk = "~/zephyr-sdk-0.17.0"
path_prepend = ["~/zephyrproject/.venv/bin"]   # python with Zephyr's requirements

[eval.capability.python]
path_prepend = ["~/.local/share/nightshift/venvs/python/bin"]   # python3 with pytest, for py-* fixtures
```

A capability is a table of arbitrary string keys plus the optional `path_prepend` list. It counts as configured when the table exists.

Trials run in scratch directories with permissions bypassed, so the harness has no access beyond the scratch copy and the throwaway home. Never point a case at a real checkout.

## Running

```
ns eval [skill...] [--case <id>] [--arms with,without | --compare <ref>]
        [--trials N] [--changed-since <ref>] [--budget-usd X] [--max-runs N]
        [--triggers-only | --cases-only] [--dry-run] [--no-cache] [--out <file>]
```

- `--harness`, `--model` and `--trigger-timeout` (default 180 s) override the config for one invocation. `--case` skips trigger evals.
- `--dry-run` prints the plan (cases, arms, trials, cached baselines reused, estimated runs) and calls no model.
- **Baseline cache.** A `without` result is keyed by (case content hash, fixture hash, harness, model, extra skills hash). A hit is reused across invocations. `--no-cache` forces a rerun.
- **Adaptive trials.** Each arm starts at `trials`. While any arm has mixed results (some trials pass, some fail), add one trial per arm, up to `max_trials`. Arms that are unanimous stop early.
- **Budget.** Stop starting new trials when cost reaches `budget_usd` or the run count reaches `max_runs`. Report everything not run as `skipped: budget`.
- **Changed skills.** `--changed-since <ref>` selects skills with any file changed since `ref`.

## Metrics

Per case and arm: pass rate, median tokens (input and output), median cost, median wall time, and turns. Per skill:

- **uplift**: pass rate `with` minus `without`, averaged over cases
- **efficiency**: median tokens and time, `with` relative to `without`
- **trigger precision and recall** from `triggers.toml`

The 2x2 reading: an accuracy gain and an efficiency gain is a clear win. Accuracy up with efficiency down is a trade. Accuracy flat with efficiency up still helps. No gain on either is a retirement candidate.

Trigger evals parse the harness transcript for the skill being loaded. `claude-stream-json` detects a `Skill` tool call naming the skill, or a read of its `SKILL.md`. Harnesses with no parser report triggers as `unsupported`.

## Results

`ns eval` writes a summary JSON per skill to `skills/ns-<skill>/evals/results/<date>-<short-sha>.json`, holding the config, per-case metrics and skill-level metrics, without transcripts. Commit these. Raw transcripts go under `[eval].transcripts`.

## Quality results

`ns quality` results (docs/FACTORY.md, Quality), newest first.

### ns-build self-check (#150), measured 2026-10-10

The self-check merged as a5ca0f3 at 2026-10-09 21:04:39 UTC. Units split by their `build.md`: the 25 built before the cut-off (2026-10-09 02:16 to 19:45 UTC) have no `## Self-check` section, and the 17 built after it (2026-10-10 02:43 to 07:46 UTC) all have one. Unit 114 is left out: it was built before the cut-off and reviewed again after it.

| First pass | Before (25 units) | After (17 units) |
|---|---|---|
| Yield (clean / units) | 0.20 (5 / 25) | 0.59 (10 / 17) |
| Blocking findings / changed lines | 55 / 7008 | 30 / 4639 |
| Per 100 changed lines | 0.78 | 0.65 |
| architecture | 14, 0.20 | 7, 0.15 |
| comments | 1, 0.01 | 1, 0.02 |
| correctness | 14, 0.20 | 9, 0.19 |
| performance | 3, 0.04 | 0, 0.00 |
| readability | 6, 0.09 | 3, 0.06 |
| security | 2, 0.03 | 2, 0.04 |
| spec | 8, 0.11 | 7, 0.15 |
| tests | 17, 0.24 | 17, 0.37 |
| unknown axis | 12, 0.17 | 5, 0.11 |

Axis rows are first-pass blocking findings, then findings per 100 changed lines. A finding that names several axes counts once under each, so the axis rows sum to 77 before and 51 after, against 55 and 30 blocking findings.

Read these with care:

- Before, 9 of the 25 units have no first-pass finding count, so the per-100-lines numbers cover 16 units. After covers all 17.
- Axes are partly guessed before. 96 of its findings have no axis and 261 take theirs from `Raised by`. After, 15 findings have no axis and none fall back.
- Review changed too. #166 (findings in unchanged code never block) merged at 23:12 UTC, after the cut-off and before every after-set review, so part of the yield gain may be review's, not build's.
- Tests findings per 100 lines rose. The self-check asks for a test on every branch, so the tests axis is the one to watch.
- `ns quality --since 2026-10-10` finds 13 units, not 17: `--since` cuts at local midnight (EDT here), and 96, 131, 133 and 172 were reviewed on the evening of 2026-10-09 local time.

Commands. These split at the merge instant on each unit's newest review, not on `build.md`, so unit 114 lands in the after set and the counts differ from the table:

```bash
ns quality --until 2026-10-09T21:04:38Z --json | jq '.first_pass, .gaps'
ns quality --since 2026-10-09T21:04:39Z --json | jq '.first_pass, .gaps'
```
