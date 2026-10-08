# Stack

How to build, test, lint and format this repo. The `sf-*` skills run these commands instead of guessing. Durations tell an agent which loop to run often and which to save for the end.

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

## Test seams

Fastest first. A seam the project doesn't have is `none`.

| Seam | Command | Takes | Needs |
|---|---|---|---|
| host unit | [e.g. `uv run pytest -m "not hil"`] | [e.g. 12 s] | nothing |
| simulator/emulator | [e.g. `west twister -p native_sim -T tests`, or `none`] | [e.g. 3 min] | [e.g. Zephyr SDK] |
| hardware-in-the-loop | [e.g. `uv run pytest -m hil --port /dev/ttyACM0`, or `none`] | [e.g. 8 min] | [e.g. nRF52840 DK on USB, J-Link] |

## Gotchas

[Anything the config files don't say: a test that needs a running service, a board that must be power-cycled after flashing, a flaky seam. Delete this section if there is nothing.]
