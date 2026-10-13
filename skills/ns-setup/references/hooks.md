# Hooks

How `ns-setup` finds a repo's pre-commit hook, and the files it proposes when there is none.

## Detection

Check in this order and stop at the first hit:

1. `git config core.hooksPath` is set: the hooks live in that directory.
2. `.githooks/` exists.
3. `.pre-commit-config.yaml` exists (pre-commit.com).
4. `.husky/` exists (husky).
5. `lefthook.yml` exists (lefthook).
6. `.git/hooks/pre-commit` exists and is executable. In a worktree, look under `git rev-parse --git-common-dir`, not `.git/`.

On a hit, record the hook in `stack.md`'s `## Hooks` section and change nothing: no new script, no new hook, no `git config`. Its `ci-local` row is the repo's existing script that runs CI locally when there is one, else `none` (plain, no backticks, so the gate finds no command).

## Proposal

With no hit, propose three things, all derived from the repo's real CI workflow (`.github/workflows/*.yml`, `.gitlab-ci.yml`). Never paste the examples below into a repo whose CI runs something else. Without a CI workflow, take the commands from the `stack.md` draft and say so.

**`scripts/ci-local.sh`**, executable:

- runs the commands CI runs, in CI's order, stopping at the first failure
- with `--fast`, runs only the quick subset: format check, lint, typecheck and the unit tests, if they finish in about 30 s. Skip a step CI doesn't run.
- with no argument, runs everything, slow seams included
- exits 2 with a usage line on any other argument
- is POSIX `sh` (`#!/bin/sh`, `set -eu`), no bashisms, and calls only tools CI already installs. It adds no runtime dependency.
- `cd`s to the repo root first, so a hook or a gate can run it from anywhere
- passes `NS_CPUS` to any command that starts its own workers (see Explore in `SKILL.md`)

**`.githooks/pre-commit`**, executable:

```sh
#!/bin/sh
# Enable with: git config core.hooksPath .githooks
exec scripts/ci-local.sh --fast
```

**`git config core.hooksPath .githooks`** for the main checkout, and the same command as a `[worktree] setup` entry in `.nightshift/nightshift.toml`, so every unit worktree runs the hook:

```toml
[worktree]
setup = ["git config core.hooksPath .githooks"]
```

The `ci-local` row of `stack.md` names `scripts/ci-local.sh`. The `ns run` gate reads that row, and runs it with `--fast` after every phase but build.

### Gates

- `gates: stop`: show the three files and the `git config` line, and wait for a yes before writing any of them.
- `gates: auto`: write `scripts/ci-local.sh` and `.githooks/pre-commit`, run the `git config` line in the main checkout, and fill the `ci-local` row and `## Hooks` section. Leave `.nightshift/nightshift.toml` alone, since many repos have its changes reviewed by a human. Give the `[worktree] setup` line in the final message for whoever merges to add.

Before handing over, run `sh -n scripts/ci-local.sh` and then `scripts/ci-local.sh --fast` once. A step that fails on today's code is a finding to report, not a reason to drop the step.

## Example: Python

CI runs `ruff check .`, `ruff format --check .`, `mypy src` and `pytest -q`, then an integration job with `pytest -q -m slow`:

```sh
#!/bin/sh
# Run what .github/workflows/ci.yml runs, in order. --fast runs the quick steps only.
set -eu
cd "$(dirname "$0")/.."

fast=0
case "${1-}" in
  --fast) fast=1 ;;
  "") ;;
  *) echo "usage: scripts/ci-local.sh [--fast]" >&2; exit 2 ;;
esac

ruff check .
ruff format --check .
mypy src
pytest -q -m "not slow"
[ "$fast" = 1 ] && exit 0
pytest -q -m slow
```

## Example: Zephyr firmware

CI runs `clang-format` over `app/` and `tests/`, the twister host subset on `native_sim`, then a board build:

```sh
#!/bin/sh
# Run what .github/workflows/ci.yml runs, in order. --fast runs the quick steps only.
set -eu
cd "$(dirname "$0")/.."

fast=0
case "${1-}" in
  --fast) fast=1 ;;
  "") ;;
  *) echo "usage: scripts/ci-local.sh [--fast]" >&2; exit 2 ;;
esac

find app tests -name '*.[ch]' -exec clang-format --dry-run --Werror {} +
west twister -p native_sim -T tests -j "${NS_CPUS:-$(nproc)}"
[ "$fast" = 1 ] && exit 0
west build -p always -b nrf52840dk/nrf52840 app
```

Twister on `native_sim` belongs in `--fast` only when it finishes in about 30 s. When it takes longer, move it after the `exit 0` line and say so in `stack.md`.
