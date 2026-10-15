# Ratchets

A ratchet holds a measured value to the one recorded for it, so a unit can't drop coverage or grow an image unnoticed (`docs/factory/run.md`, "Ratchets"). The Python, Rust and C rule packs each ship an example: a script that prints one number, and its `[ratchets.*]` tables in the pack's `ratchet.toml`.

## Which fit

Offer only the ones whose evidence Explore found. A repo with none of these gets no ratchet offer.

| Pack | Ratchet | Fits when | Placeholders |
|---|---|---|---|
| `rules/python/` | `coverage`, higher is better | the Python pack is taken and the tests run under pytest | none; append pytest arguments such as `--cov=<package>` to the command |
| `rules/rust/` | `binary-size`, lower is better | a `Cargo.toml` with a binary target (`src/main.rs`, `src/bin/`, a `[[bin]]`) | `{bin}` the binary, `{crate}` its crate's directory, relative to the repo |
| `rules/c/` | `flash` and `ram`, lower is better | the C pack is taken and a build writes an ELF image (Zephyr's `zephyr/zephyr.elf`) | `{build}` the command that builds the image, `{elf}` the image it writes |

## Install

Run these after the pack itself is installed, since each command calls the copy under `.nightshift/rules/<pack>/`.

1. **Check the tool.** Python: `pytest -VV` lists `pytest-cov`; a missing one: give the user the command that adds it to the repo's dev dependencies and wait until `pytest -VV` lists it. Rust: `cargo` and `jq` on PATH. C: the image's `size` tool on PATH; a cross build names its own (`arm-zephyr-eabi-size`) through `SIZE=` before the script in the command.
2. **Write the tables.** Append the pack's `ratchet.toml` tables, comment included, to `.nightshift/nightshift.toml`, with each placeholder filled from what Explore found. Leave `ratchets.json` unwritten: recording the values is the human's act.
3. **Run it.** `ns ratchet check --human`. Each ratchet reads `untracked` with a measured number. A `broken` or `flaky` one needs its command fixed before the user records it.
4. **Hand it over.** Tell the user that `.nightshift/nightshift.toml` needs their review, and that `ns ratchet update` records today's values in `ratchets.json`, theirs to run and commit.

Done when each ratchet the user took is a table in `nightshift.toml` with no placeholder left, `ns ratchet check` reports it `untracked` with a number, and the user has the `ns ratchet update` command.
