# Scorers

A scorer grades a finished unit against a rubric (D18 in `docs/decisions/`). Each is one file, `scorers/<name>.md`; the name is the file stem.

```markdown
---
role: review.scorer        # the `ns ask` role whose model applies the rubric
passing_score: 1           # a unit passes when its label's score is at least this
sampling_rate: 0.25        # the share of units to grade, 0 to 1
labels:                    # the answers the model may give
  - value: pass
    description: every acceptance criterion has evidence
    score: 1
  - value: fail
    description: a criterion has no evidence
    score: 0
---
Read the unit's brief.md and evidence.md. Answer `pass` if ...
```

The body below the frontmatter is the rubric. `ns factory validate` rejects a scorer with missing or malformed frontmatter (including unknown keys), no labels, a label without `value` or `score`, a missing `passing_score` or one above every label's score, a missing `sampling_rate` or one outside 0 to 1, a `role` that the user config does not resolve for `ns ask` (no config file counts as not resolved), or an empty rubric. `description` is optional. `sampling_rate` is read and checked but not yet acted on: `ns score` grades every unit it is given.

## Scoring

`ns score <unit>` grades one finished unit with every valid scorer, or only the one `--scorer <name>` names. For each scorer it builds a prompt from:

- the rubric and the labels with their scores;
- every `*.md` file directly in `.ns/<unit>/`, read from the unit's worktree when it has one, else from the main checkout;
- the PR outcome, as JSON: the PR number from `pr.md` (or `{"pr": null}` without one), its `state` from `gh pr view`, `reverted` (a commit on the default branch says `This reverts commit <merge commit>`), and `review_rounds` (how many reviews the PR got).

It runs the scorer's `role` read-only through the `ns ask` path, in the current directory, and takes the reply's last `label: <value>` line. The value must be one of the scorer's labels; its score is the label's score, and `pass` is `score >= passing_score`. A reply that names no known label is an error (exit 1). Nothing is appended unless every scorer's judge named a known label, so a failed run can be repeated.

The run ledger is `ns/scores.jsonl` in the common git dir (`.git/ns/scores.jsonl` in the main checkout), one JSON line per graded scorer, appended:

```json
{"unit":"142-uart-timeout","scorer":"evidence","label":"pass","score":1.0,"pass":true}
```

Running `ns score` twice on a unit appends two lines; a reader takes the last line per unit and scorer. `ns score` prints `{ok, unit, ledger, scores}`. `--dry-run` prints `{ok, dry_run, unit, prompts: [{scorer, role, prompt}]}`, runs no judge and writes no ledger line; it still reads the PR with `gh`. Errors follow the `ns` exit codes: 1 for a judge or `gh` failure, an unknown label, or invalid or missing scorers; 2 for a bad unit id or an unknown `--scorer`; 3 and 4 as for `ns ask`.
