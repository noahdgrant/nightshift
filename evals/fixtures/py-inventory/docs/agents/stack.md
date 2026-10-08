# Stack

How to build, test, lint and format this repo. The `ns-*` skills run these commands instead of guessing. Durations tell an agent which loop to run often and which to save for the end.

## Languages

- Python 3.11+, src layout (`src/inventory/`), no runtime dependencies

## Setup

`python3 -m pip install pytest` (pytest is the only dev dependency; `pyproject.toml` puts `src/` on the test path, so no install of the package is needed)

## Commands

| Task | Command | Takes |
|---|---|---|
| build | none | |
| test (all host tests) | `python3 -m pytest -q` | 1 s |
| test (one test) | `python3 -m pytest -q tests/test_warehouse.py::test_cancel_returns_stock` | 1 s |
| lint | `ruff check .` (optional, if ruff is installed) | 1 s |
| format | `ruff format .` (optional, if ruff is installed) | 1 s |
| typecheck | none | |

## Test seams

Fastest first. A seam the project doesn't have is `none`.

| Seam | Command | Takes | Needs |
|---|---|---|---|
| host unit | `python3 -m pytest -q` | 1 s | pytest |
| simulator/emulator | none | | |
| hardware-in-the-loop | none | | |

Public seams: the `Warehouse` class (exported from `inventory`), `inventory.storage.load`/`save`, and the CLI through `inventory.cli.main(argv)`.

## Gotchas

Time-dependent methods take `now` explicitly. Tests pass fixed datetimes; never sleep or read the wall clock in a test.
