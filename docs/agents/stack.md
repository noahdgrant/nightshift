# Stack

How to build, test, lint and format this repo. The `ns-*` skills run these commands instead of guessing.

## Languages

- Rust (stable, 2021 edition) for the `ns` CLI in `cli/`
- Markdown skills in `skills/ns-*/`, with TOML eval cases in `skills/*/evals/`
- Python 3.11+ and C (Zephyr) only inside the eval fixtures under `evals/fixtures/`
- The rule packs under `rules/`, listed in `rules/README.md`. Two need a toolchain of their own for their tests:
  - JavaScript and TypeScript only in `rules/js/`, whose tests need Node 18 and npm (`scripts/ci-tools.d/`)
  - Java 11+ and PMD only for `rules/java/`'s tests (`rules/java/tests/run.sh`, `cli/tests/rules_pack_java.rs`); `scripts/ci-tools.sh install pmd` fetches the pinned PMD
- `scripts/ci-tools.sh` needs Python 3.11+ on PATH (it reads `tomllib`) and fails with a message naming it otherwise

## Setup

`cd cli && cargo build`. A fresh clone needs nothing else.

## Commands

Run from the repo root. Use the binary built from this worktree (`cli/target/debug/ns`), never a globally installed `ns`, when checking CLI changes.

| Task | Command | Takes |
|---|---|---|
| build | `cargo build --manifest-path cli/Cargo.toml` | 5 s incremental, 60 s clean |
| test (all) | `cargo test --manifest-path cli/Cargo.toml`. Build runs it only inside the gate (`ci-local`), never on its own | 8 min |
| test (one) | `cargo test --manifest-path cli/Cargo.toml <name>` | 2 s |
| lint | `cargo clippy --manifest-path cli/Cargo.toml --all-targets -- -D warnings` | 10 s |
| format | `cargo fmt --manifest-path cli/Cargo.toml` (check: `--check`) | 1 s |
| skill lint | `cli/target/debug/ns lint skills --human` and `cli/target/debug/ns lint .agents/skills --human` (repo-local skills, no `ns-` prefix required) | 1 s |
| eval cases valid | `cli/target/debug/ns eval --dry-run` | 1 s |
| private refs | `scripts/check-private.sh` | 1 s |
| verify recipes | `scripts/check-verify-recipes.sh` (fails naming the file and block of any `.agents/skills/verify-nightshift` recipe line that does not run through `"$V/sandbox"`, uses a glob, or names a `.ns/` path other than one unit's folder; then runs the skill's Launch, Doctor and Drive recipes and `features/run.md`'s in a throwaway sandbox with the fake `claude` and `gh`, and fails naming a stale block, a bad Doctor result, or an `ns run --issue 7` that does not end `done`; needs `jq`) | 15 s |
| rules | `rules/rust/check.sh clippy cli`, `rules/rust/check.sh deny cli`, `rules/rust/check.sh ast-grep cli` (the Rust rule pack; needs `jq`, `ast-grep` and `cargo-deny`) | 10 s |
| ci-local | `scripts/ci-local.sh` (all of CI). It unsets git's location variables (`GIT_DIR` and the rest) first, so a run from a hook checks this checkout (ADR-638). It fails first on a tool not pinned in `scripts/ci-tools.d/` or at another version. Under `[gate] local = "build-only"` only the `ns run` gate after build runs all of it; the gates after verify and review run `--fast`, none runs after ship (GitHub checks decide in the merge step), and pre-commit and the `.githooks` pre-push hook (at each commit pushed, one push at a time) run its `--fast` subset, which includes the `rules` row's ast-grep commands (`ns rules check --only ast-grep`, about 0.8 s) | 13 min |
| ci-affected | `scripts/ci-affected.sh` (the tests for the paths in `NS_GATE_FILES`). cargo test runs only the test binaries `scripts/ci-affected.map` lists for a changed path, and all of it for a path under `cli/`, `Cargo.lock`, the scripts that run the tests or pin their tools, or the map; a path no entry lists runs none (a `docs/` change runs `--bin ns`, about 35 s with its build). `cli/tests/ci_local/affected_map.rs` fails when a test reads a repo path the map doesn't list for it. The `rules` row's ast-grep commands (`ns rules check --only ast-grep`, about 0.5 s) run on any change, and each rule pack's or fixture's tests when a path under it changed. `NS_GATE_FILES` unset, or holding a path that is not a plain repo path, runs everything. The fast gate runs it after `ci-local.sh --fast`. On a pull request GitHub CI runs its cargo test step (`--cargo-test`) and asks it which packs and fixtures changed (`--changed <prefix>`), with the PR's paths from `scripts/ci-pr-files.sh` | 35 s to 7 min |

Timings for `test (all)`, `ci-local` and `ci-affected` were measured on 2026-10-14 on a 16-core machine with `scripts/stress.sh` not running, but with another unit's phase running alongside (load average 3 to 8): `ci-local --no-cache` took 754 s, its `cargo test` step 482 s with the test build, and `ci-affected` for a skills change 396 s with a warm build, before it picked binaries from the map. On a busier machine expect up to twice that.

## Test seams

| Seam | Command | Takes | Needs |
|---|---|---|---|
| host unit | `cargo test --manifest-path cli/Cargo.toml` | 8 min | nothing |
| CLI end to end | the integration tests in `cli/tests/` (fake harness, fake `gh`, temp git repos) | included above | nothing |
| simulator/emulator | none | | |
| hardware-in-the-loop | none | | |

## Unit size

unit size: soft 400, split 1200

The split threshold comes from `.git/ns/runs.jsonl` and the merged PRs: 120 units through 2026-10-14, timed from the unit's start to its merge. Median lead time stays between 50 and 70 minutes up to 1,200 changed lines, and jumps to 318 minutes past it (#647). See `ns-contract` (unit size).

## Gotchas

- Never run a real `ns eval` (without `--dry-run`) or `ns run` / `ns watch` inside a phase. They start model runs that consume the plan's usage. Eval runs are scheduled separately.
- A skill change runs `ns lint` and comes with eval cases or a stated reason it has none (`AGENTS.md`).
- Committed files carry no private company names or real home paths. `scripts/check-private.sh` and CI enforce this.
- The test harness in `cli/tests/common/` needs Linux: `/proc`, `ps` and util-linux `setsid`. Tests that start `ns` or a helper kill its whole process tree, so they do not run on macOS or Windows.
