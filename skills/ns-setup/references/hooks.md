# Hooks

How `ns-setup` finds a repo's pre-commit hook, the files it proposes when there is none, and the `[merge] human_review` list it proposes in every repo.

## Detection

Check in this order and stop at the first hit:

1. `git config --local core.hooksPath` is set (a global value does not count) and that directory holds an executable `pre-commit`. Otherwise continue down the list.
2. `.githooks/pre-commit` exists.
3. `.pre-commit-config.yaml` exists (pre-commit.com).
4. `.husky/pre-commit` exists (husky).
5. `lefthook.yml` exists and has a `pre-commit:` key (lefthook).
6. `.git/hooks/pre-commit` exists and is executable. In a worktree, look under `git rev-parse --git-common-dir`, not `.git/`.

On a hit, change no files: no new script, no new hook, no `git config`. Record the hook in `stack.md`'s `## Hooks` section. If it is not enabled (steps 2 to 5 with `core.hooksPath` unset or not pointing at it, or `pre-commit install` / `husky` / `lefthook install` not run), record "present, not enabled" with the enabling command, and name that command in the final message. Never run it for a pre-existing hook. If the hit is our own partial setup (`.githooks/pre-commit` calling `scripts/ci-local.sh --fast`, `core.hooksPath` unset), say to finish by running `git config core.hooksPath .githooks`.

The `ci-local` row is the command of the repo's existing script that runs CI locally (`make ci`, `scripts/test-all.sh`), whatever flags it takes. Only `[gate] local = "build-only"` appends ` --fast` to it; the default `every-step` runs it as written, so a script with no `--fast` is fine there. Say in the row whether it accepts `--fast`, since `ns factory validate` warns under `build-only` when the row never names `--fast`. With no such script the row is `none` (plain, no backticks, so the gate finds no command). When a script that runs CI already exists, never overwrite it: propose only `.githooks/pre-commit` and the `git config` line. The hook runs the script with `--fast` only when it accepts it. Otherwise it runs a quick command the script doesn't cover (the lint or unit tests from the `stack.md` draft), or, when no step is quick, leave the hook out and say so.

## Proposal

With no hit, propose three things, all derived from the repo's real CI workflow (`.github/workflows/*.yml`, `.gitlab-ci.yml`). Never paste the examples below into a repo whose CI runs something else. Without a CI workflow, take the commands from the `stack.md` draft and say so.

**`scripts/ci-local.sh`**, executable:

- runs the commands CI runs, in CI's order, stopping at the first failure
- with `--fast`, runs only the quick subset: format check, lint, typecheck and the unit tests, if they finish in about 30 s. Skip a step CI doesn't run.
- with no argument, runs everything, slow seams included, and exits 0 when no slow step follows the quick ones
- exits 2 with a usage line on any other argument
- is POSIX `sh` (`#!/bin/sh`, `set -eu`), no bashisms, and calls only tools CI already installs. It adds no runtime dependency.
- `cd`s to the repo root first, so a hook or a gate can run it from anywhere
- mirrors only verification steps (format, lint, typecheck, test, build). Never copy a step that deploys, publishes, releases, pushes an image or package, flashes hardware, or needs a secret or token: leave it out and say so in `stack.md`.
- passes `NS_CPUS` to any command that starts its own workers (see Explore in `SKILL.md`)

**`.githooks/pre-commit`**, executable:

```sh
#!/bin/sh
# Enable with: git config core.hooksPath .githooks
exec scripts/ci-local.sh --fast
```

**`git config core.hooksPath .githooks`** for the main checkout, set last, and the same command as a `[worktree] setup` entry in `.nightshift/nightshift.toml`, so every unit worktree runs the hook:

```toml
[worktree]
setup = ["git config core.hooksPath .githooks"]
```

The `ci-local` row of `stack.md` names `scripts/ci-local.sh`. The `ns run` gate reads that row, and runs it plain after build. Under `[gate] local = "build-only"` it runs it with `--fast` after every other phase; under the default `every-step` it runs it plain each step.

### Gates

- `gates: stop`: show the three files and the `git config` line, and wait for a yes before writing any of them. After the yes, write the `[worktree] setup` entry to `.nightshift/nightshift.toml`, appended to the existing `setup` array when the worktree trap fix-ups write one too.
- `gates: auto`: write `scripts/ci-local.sh` and `.githooks/pre-commit`, run the `git config` line in the main checkout last, after the files exist, and fill the `ci-local` row and `## Hooks` section. Leave `.nightshift/nightshift.toml` alone, since many repos have its changes reviewed by a human. Give the `[worktree] setup` line in the final message for whoever merges to add.

`ns-setup`, run by the user to onboard a repo, is the sanctioned writer of `core.hooksPath` and `.githooks/`, which `skills/ns-contract` (Git hooks) forbids to phases. Never do this inside an `ns run` phase: if the guard denies the write, report it, and do not retry or bypass it.

Before handing over, run `sh -n scripts/ci-local.sh` and then `scripts/ci-local.sh --fast` once. A step that fails on today's code is a finding to report, not a reason to drop the step.

## Merge review list

`[merge] human_review` in `.nightshift/nightshift.toml` lists the globs whose changes need a human merge (`docs/factory/merge.md`). Empty, `policy = "auto"` lets `ns run` merge changes to the factory's own guardrails with no human. Propose a starter list in every run, built from paths the repo has:

- the CI config: `.github/**` on GitHub, `.gitlab-ci.yml` on GitLab (the tracker Section D found)
- the hook directory: `.githooks/**`, or the hook found in Detection (`.pre-commit-config.yaml`, `.husky/**`, `lefthook.yml`)
- the local CI script: the `ci-local` row of `stack.md`, such as `scripts/ci-local.sh`
- `.nightshift/**`, always

List only paths that exist, counting the files Section B writes, so a repo with none of them gets `[".nightshift/**"]`. When the file already has a `human_review`, keep every entry the owner wrote and add only the missing ones. Leave `policy` as it is.

```toml
[merge]
human_review = [".github/**", ".githooks/**", "scripts/ci-local.sh", ".nightshift/**"]
```

- `gates: stop`: show the list and let the owner edit it. After a yes, write it to `.nightshift/nightshift.toml`. On a no, write nothing and tell the owner the repo keeps an empty `human_review`.
- `gates: auto`: leave `.nightshift/nightshift.toml` alone, as Gates says, and give the `[merge]` block in the final message for whoever merges to add.

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
if [ "$fast" = 1 ]; then exit 0; fi
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
if [ "$fast" = 1 ]; then exit 0; fi
west build -p always -b nrf52840dk/nrf52840 app
```

Twister on `native_sim` belongs in `--fast` only when it finishes in about 30 s. When it takes longer, move it after the `exit 0` block and say so in `stack.md`.
