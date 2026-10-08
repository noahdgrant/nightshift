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
commands = ["mkdir -p .ns/7-reserve-overflow", "cp ../brief.md .ns/7-reserve-overflow/"]

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
- `{changed_tests}` expands to the test files the trial added or changed relative to the base commit.
- A trial **passes** when every required check passes. Checks are required by default, except `judge`.
- `fails_on_base` copies the changed test files onto a clean checkout of the base commit, runs the command there, and passes when it exits non-zero. It proves the tests can go red on the bug.
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

## Arms and isolation

Each case runs in arms:

- `with`: `skills` + `extra_skills` installed.
- `without`: `extra_skills` only.
- `--compare <ref>`: replaces `without` with the case's skills as they were at a git ref (`old` vs `new`).

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

[eval.harness.claude]
command = ["claude", "-p", "--model", "{model}", "--output-format", "stream-json", "--verbose", "--permission-mode", "bypassPermissions"]
carry = [".claude/.credentials.json", ".claude.json"]
output = "claude-stream-json"

[eval.capability.zephyr]
base = "~/prevasum/embedded-workspace/zephyr"
sdk = "~/zephyr-sdk-0.17.0"
```

Trials run in scratch directories with permissions bypassed, so the harness has no access beyond the scratch copy and the throwaway home. Never point a case at a real checkout.

## Running

```
ns eval [skill...] [--case <id>] [--arms with,without | --compare <ref>]
        [--trials N] [--changed-since <ref>] [--budget-usd X] [--max-runs N]
        [--triggers-only | --cases-only] [--dry-run] [--no-cache] [--out <file>]
```

- `--dry-run` prints the plan (cases, arms, trials, cached baselines reused, estimated runs) and calls no model.
- **Baseline cache.** A `without` result is keyed by (case content hash, fixture hash, harness, model, extra skills hash). A hit is reused across invocations. `--no-cache` forces a rerun.
- **Adaptive trials.** Each arm starts at `trials`. If the arms disagree within a case (any trial differs), or the pass-rate gap is under 0.5, add one trial per arm up to `max_trials`.
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
