# Stack

How to build, test, lint and format this repo. The `ns-*` skills run these commands instead of guessing. Durations tell an agent which loop to run often and which to save for the end.

## Languages

- [e.g. Python 3.12, managed with uv]
- [e.g. C (Zephyr 3.7) for the firmware in `app/`]

## Setup

[One command that installs dependencies and toolchains on a fresh clone, e.g. `uv sync`, `west update`.]

## Commands

| Task | Command | Takes |
|---|---|---|
| build | [e.g. `west build -b nrf52840dk/nrf52840 app`] | [e.g. 40 s] |
| test (all host tests) | [e.g. `uv run pytest`] | [e.g. 12 s] |
| test (one test) | [e.g. `uv run pytest tests/test_uart.py::test_timeout`] | [e.g. 1 s] |
| lint | [e.g. `uv run ruff check .`] | |
| format | [e.g. `uv run ruff format .`] | |
| typecheck | [e.g. `uv run mypy src`, or `none`] | |
| rules | [the rule pack's commands, or `none`] | |
| ci-local | [e.g. `scripts/ci-local.sh`: all of CI, in CI's order; `--fast` runs the quick subset. `none` (no backticks) when the repo has no such script] | [e.g. 2 min, `--fast` 20 s] |

## Test seams

Fastest first. A seam the project doesn't have is `none`.

| Seam | Command | Takes | Needs |
|---|---|---|---|
| host unit | [e.g. `uv run pytest -m "not hil"`] | [e.g. 12 s] | nothing |
| simulator/emulator | [e.g. `west twister -p native_sim -T tests`, or `none`] | [e.g. 3 min] | [e.g. Zephyr SDK] |
| hardware-in-the-loop | [e.g. `uv run pytest -m hil --port /dev/ttyACM0`, or `none`] | [e.g. 8 min] | [e.g. nRF52840 DK on USB, J-Link] |

## Hooks

[The pre-commit hook and how it is enabled, e.g. `.githooks/pre-commit` runs `scripts/ci-local.sh --fast`, enabled with `git config core.hooksPath .githooks`; or the existing `.pre-commit-config.yaml` (pre-commit.com), enabled with `pre-commit install`. `none` if the repo has no hook.]

## Unit size

unit size: soft 400

See `ns-contract` (unit size).

## Gotchas

[Anything the config files don't say: a test that needs a running service, a board that must be power-cycled after flashing, a flaky seam. Delete this section if there is nothing.]
