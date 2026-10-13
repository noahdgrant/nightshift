# `ns score`

## `ns score <unit-id> [--scorer <name>] [--factory <dir>] [--dry-run]`

Grades a finished unit with each scorer in `scorers/` (or the one `--scorer` names): builds a prompt from the rubric and labels, the `*.md` files in `.ns/<unit>/` and the PR outcome from `gh`, runs the scorer's role read-only through the `ns ask` path, and maps the reply's last `label: <value>` line to a score. Appends `{unit,scorer,label,score,pass}` per scorer to `ns/scores.jsonl` in the common git dir, and nothing when any judge names no known label (exit 1). `--dry-run` prints each prompt and writes nothing, but still calls `gh`. See `docs/factory/scorers.md`, "Scoring".
