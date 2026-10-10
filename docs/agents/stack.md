# Stack

How to build, test, lint and format this repo. The `ns-*` skills run these commands instead of guessing.

## Languages

- Rust (stable, 2021 edition) for the `ns` CLI in `cli/`
- Markdown skills in `skills/ns-*/`, with TOML eval cases in `skills/*/evals/`
- Python 3.11+ and C (Zephyr) only inside the eval fixtures under `evals/fixtures/`

## Setup

`cd cli && cargo build`. A fresh clone needs nothing else.

## Commands

Run from the repo root. Use the binary built from this worktree (`cli/target/debug/ns`), never a globally installed `ns`, when checking CLI changes.

| Task | Command | Takes |
|---|---|---|
| build | `cargo build --manifest-path cli/Cargo.toml` | 5 s incremental, 60 s clean |
| test (all) | `cargo test --manifest-path cli/Cargo.toml` | 5 s |
| test (one) | `cargo test --manifest-path cli/Cargo.toml <name>` | 2 s |
| lint | `cargo clippy --manifest-path cli/Cargo.toml --all-targets -- -D warnings` | 10 s |
| format | `cargo fmt --manifest-path cli/Cargo.toml` (check: `--check`) | 1 s |
| skill lint | `cli/target/debug/ns lint skills --human` | 1 s |
| eval cases valid | `cli/target/debug/ns eval --dry-run` | 1 s |
| private refs | `scripts/check-private.sh` | 1 s |
| ci-local | `scripts/ci-local.sh` (all of CI, including `scripts/check-git-dir-decoy.sh`, which runs `cargo test` under a decoy `GIT_DIR`; `--fast` for fmt, clippy, ns lint and the private guard) | 1 min |

## Test seams

| Seam | Command | Takes | Needs |
|---|---|---|---|
| host unit | `cargo test --manifest-path cli/Cargo.toml` | 5 s | nothing |
| CLI end to end | the integration tests in `cli/tests/` (fake harness, fake `gh`, temp git repos) | included above | nothing |
| simulator/emulator | none | | |
| hardware-in-the-loop | none | | |

## Unit size

unit size: soft 400

See `ns-contract` (unit size).

## Gotchas

- Never run a real `ns eval` (without `--dry-run`) or `ns run` / `ns watch` inside a phase. They start model runs that consume the plan's usage. Eval runs are scheduled separately.
- A skill change runs `ns lint` and comes with eval cases or a stated reason it has none (`AGENTS.md`).
- Committed files carry no private company names or real home paths. `scripts/check-private.sh` and CI enforce this.
- The test harness in `cli/tests/common/` needs Linux: `/proc`, `ps` and util-linux `setsid`. Tests that start `ns` or a helper kill its whole process tree, so they do not run on macOS or Windows.
