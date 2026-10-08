# Stack

How to build, test, lint and format this repo. The `ns-*` skills run these commands instead of guessing. Durations tell an agent which loop to run often and which to save for the end.

## Languages

- C (Zephyr 4.3), packaged as an out-of-tree Zephyr module named `zlogger` (`zephyr/module.yml`)
- CMake and Kconfig for the module build

## Setup

No `west.yml`: this repo is a module, not a workspace. It builds against an existing Zephyr tree:

- `ZEPHYR_BASE` points at that tree (e.g. `~/zephyrproject/zephyr`).
- `ZEPHYR_SDK_INSTALL_DIR` points at the Zephyr SDK (e.g. `~/zephyr-sdk-0.17.0`). `native_sim` compiles with the host `gcc` (32-bit, so `gcc-multilib` must be installed).
- `python3` on `PATH` has Zephyr's `scripts/requirements-base.txt` installed (twister, west).

Run every command below from the repo root.

## Commands

| Task | Command | Takes |
|---|---|---|
| build | `west build -b native_sim app -d "$(mktemp -d)"` | 11 s |
| test (all host tests) | `"$ZEPHYR_BASE/scripts/twister" -T tests/ -p native_sim -x=ZEPHYR_EXTRA_MODULES="$PWD" -O "$(mktemp -d)" --inline-logs` | 23 s |
| test (all, wrapper) | `scripts/twister.sh` (same command, output in a fresh temp dir) | 23 s |
| test (one suite) | `scripts/twister.sh tests/lib/ringbuf_log` | 13 s |
| test (suites holding given files) | `scripts/twister.sh tests/lib/ringbuf_log/src/main.c` | 13 s |
| lint | none | |
| format | none | |
| typecheck | none (the compiler is the check) | |

## Test seams

Fastest first. A seam the project doesn't have is `none`.

| Seam | Command | Takes | Needs |
|---|---|---|---|
| host unit | ztest suites built for `native_sim` and run on the host: `scripts/twister.sh` | 23 s | Zephyr tree, host gcc |
| simulator/emulator | the same `native_sim` run; the app also runs as `<build dir>/zephyr/zephyr.exe` | 11 s build | Zephyr tree, host gcc |
| hardware-in-the-loop | none | | |

The sensor reader is tested through `struct zl_sensor_hal`: tests pass a fake HAL with a fake sensor and a fake clock (`tests/drivers/sensor_reader/src/main.c`). New sensor behavior goes behind that HAL so it stays testable on the host.

## Gotchas

- **Build this checkout, not another copy.** Zephyr finds modules through the west workspace that `ZEPHYR_BASE` sits in, not through the current directory. Without `-x=ZEPHYR_EXTRA_MODULES="$PWD"`, a twister run either fails on unknown `CONFIG_ZL_*` symbols or, if another `zlogger` copy is registered in that workspace, silently compiles that copy's sources and passes. Each test's `CMakeLists.txt` appends its own module root and aborts if `zlogger` resolves anywhere else, so a wrong-tree build fails loudly. Check by path when in doubt: the `zephyr_modules.txt` in a test's build dir must name this checkout.
- **Never write twister output into the repo.** Always pass `-O` with a temp dir (the wrapper does). A `twister-out/` in the tree shows up in the diff.
- `west twister` works too when `ZEPHYR_BASE` sits in a west workspace, but plain `$ZEPHYR_BASE/scripts/twister` needs no workspace and is the reference.
- Don't pass `-T` a file: twister takes suite directories. Use `scripts/twister.sh` to go from files to suites.
