# Run

`ns run` drives one unit through triage, build, verify, review and ship unattended. It picks the next phase from the unit's artifacts, retries a phase up to its attempts, and ends `done`, `merged` or `stuck`, logging each event to the run log.

## Sub-features

- `run-dry` prints the next decision and the rendered prompt, and runs nothing.
- `run-pass` runs all five phases and ends `done`.
- `run-stuck` ends `stuck` when a phase fails every attempt.
- `run-log` records each phase in `ns log` and `ns status`.

## How to get to it (user POV)

- `ns run --issue <n> [--dry-run]`
- `ns run <unit-id> [--from <phase>] [--gates stop|auto]`

## Driving it with ns

Preconditions:

- Baseline, in `$V/repo`. Write the factory file: `mkdir -p "$V/repo/.nightshift" && printf 'name = "demo"\n' > "$V/repo/.nightshift/nightshift.toml"`.
- Issue #7 seeded as in the skill's Drive section.

- **Dry run.** Run `cap run-dry "$NS" run --issue 7 --dry-run`. `unit` is `7-fix-the-thing`, `decision.phase` is `triage` with `why` `no brief.md`, and `prompt` starts with `Load the \`ns-triage\` skill`. `$FAKE_CTRL/calls` does not exist.
- **Pass.** Run `cap run-pass "$NS" run --issue 7`. Exit `0`, `outcome` `done`. `$FAKE_CTRL/calls` lists triage, build, verify, review, ship, and `ls "$V/repo.worktrees/7-fix-the-thing/.ns/7-fix-the-thing"` shows `brief.md`, `build.md`, `evidence.md`, `review.md`, `pr.md`.
- **Stuck.** In a fresh sandbox, run `printf 'fail\nfail\n' > "$FAKE_CTRL/build"`, then `cap run-stuck "$NS" run --issue 7`. Exit `1`, `outcome` `stuck`, reason `build is out of attempts (2)`; `$FAKE_CTRL/calls` ends with two `build` lines.
- **Log.** Run `cap run-log "$NS" log` and `cap run-log-status "$NS" status`. The log contains the line `end #7 ... outcome done ...` (the last line is `quality_record #7 status pushed`); status counts the unit under that outcome.

## Gotchas

- Doctor's fake check comes first: `ns run` with the real `claude` on PATH starts real model sessions.
- `ns run` itself does not move labels; [watch](./watch.md) does. Check labels there.
- A second run of the same unit resumes from its artifacts. Start a new sandbox to drive a phase from scratch, or use `--from`.
