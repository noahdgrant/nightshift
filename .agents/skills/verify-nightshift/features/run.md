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

- Baseline. Issue #7 seeded as in the skill's Drive section, and no `7-fix-the-thing` or `8-break-the-thing` worktree yet.
- The factory file:

  ```bash
  "$V/sandbox" sh -c 'mkdir -p .nightshift && echo "$1" > .nightshift/nightshift.toml' _ 'name = "demo"'
  ```

- **Dry run.** `unit` is `7-fix-the-thing`, `decision.phase` is `triage` with `why` `no brief.md`, and `prompt` starts with `Load the \`ns-triage\` skill`. `run-dry-calls` exits non-zero: `$V/ctrl/calls` does not exist.

  ```bash
  "$V/sandbox" cap run-dry ns run --issue 7 --dry-run
  "$V/sandbox" cap run-dry-calls cat "$V/ctrl/calls"
  ```

- **Pass.** Exit `0`, `outcome` `done`. `run-pass-calls` lists triage, build, verify, review, ship, and `run-pass-artifacts` shows `brief.md`, `build.md`, `evidence.md`, `review.md`, `pr.md`.

  ```bash
  "$V/sandbox" cap run-pass ns run --issue 7
  "$V/sandbox" cap run-pass-calls cat "$V/ctrl/calls"
  "$V/sandbox" cap run-pass-artifacts ls "$V/repo.worktrees/7-fix-the-thing/.ns/7-fix-the-thing"
  ```

- **Stuck.** Seed issue #8 and queue two build failures for its unit only, then run it. Exit `1`, `outcome` `stuck`, reason `build is out of attempts (2)`; `run-stuck-calls` ends with two `build` lines.

  ```bash
  "$V/sandbox" sh -c 'echo "$1" > "$V/ghd/issue-8.json"' _ '{"number":8,"title":"Break the thing","url":"https://github.com/o/r/issues/8","state":"OPEN"}'
  "$V/sandbox" sh -c 'printf "fail\nfail\n" > "$V/ctrl/build.8-break-the-thing"'
  "$V/sandbox" cap run-stuck ns run --issue 8
  "$V/sandbox" cap run-stuck-calls cat "$V/ctrl/calls"
  ```

- **Log.** The log contains `end #7 ship outcome done` and `end #8 build outcome stuck reason: build is out of attempts (2)`; status lists #7 under `PR for a human` and #8 under `stuck`.

  ```bash
  "$V/sandbox" cap run-log ns log
  "$V/sandbox" cap run-log-status ns status
  ```

## Gotchas

- Doctor's fake check comes first: `ns run` with the real `claude` on PATH starts real model sessions.
- `ns run` itself does not move labels; [watch](./watch.md) does. Check labels there.
- A second run of the same unit resumes from its artifacts. Start a new sandbox to drive a phase from scratch, or use `--from`.
- A queue file `$V/ctrl/<phase>` without a unit suffix applies to every unit's phase, so a later recipe's run would pop it. Queue per unit: `$V/ctrl/<phase>.<unit-id>`.
- `scripts/check-verify-recipes.sh` runs these recipes in CI after the skill's Drive, and fails unless `run-pass` ends `done`.
