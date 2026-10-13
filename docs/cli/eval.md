# `ns eval`

## `ns eval [skill...] [options]`

Implements [docs/EVALS.md](../EVALS.md): trigger evals from `skills/<skill>/evals/triggers.toml` and behaviour cases from `skills/<skill>/evals/cases/<id>/case.toml`, run against fixtures in `evals/fixtures/<name>/`. With no skill named, it takes every skill that has an `evals/` directory. `tdd` and `ns-tdd` both work.

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
