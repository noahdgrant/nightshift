# Python rule pack

The rules `ns rules check` runs on a Python repo. They compose `ruff`, `ast-grep` and a small script, and each one enforces a principle or quality-bar item. Rules no tool can express stay in review.

| Rule | Tool | Fails on | Enforces |
|---|---|---|---|
| `suppression-reason` | ast-grep | a suppression (`noqa`, `type: ignore`, `pyright: ignore`, `pylint: disable`, `ruff: noqa`, `flake8: noqa`) with no reason after it on the same line | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no suppression hides a real bug |
| `dependency-allowlist` | ast-grep | an import from outside the standard library and the allowlist | [pave the road](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-principle-pave-the-road/SKILL.md): one road per job |
| `duplicate-helper` | ast-grep | a function or class named like a helper that already exists | the same |
| `F401` | ruff | an import nothing uses | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): nothing is dead: no unused code, parameters or imports |
| `F821` | ruff | a name that is never defined | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): the diff's code runs, which a misspelt name breaks |
| `F841` | ruff | a local variable assigned and never used | the same as `F401` |
| `B006` | ruff | a mutable default argument (`def f(x=[])`) | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): shared state is guarded by structure, and a mutable default is shared by every call |
| `RUF100` | ruff | a `noqa` that suppresses nothing | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no suppression hides a real bug |
| `file-size` | `file-size.sh` | a file whose last non-blank line is past line 1000, blank lines at the top included | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no file grows past roughly 1000 lines |

A suppression's reason is a second comment after it:

```python
import os  # noqa: F401  # re-exported for callers
```

The regex only checks that a second comment follows the suppression, so any text there counts as its reason. Whether the reason is a good one stays in review.

## Install

The `ns-setup` skill installs the pack, and [its steps](../../skills/ns-setup/references/rules-python.md) work by hand too. It copies `ruff.toml`, `sgconfig.yml`, `file-size.sh`, `rules/` and `utils/` to `.nightshift/rules/python/` in the target repo and adds the row in `stack-row.md` to `docs/agents/stack.md`, with `{pack}` and `{target}` filled in. `tests/run.sh` runs the commands of that same file. `.nightshift/**` needs a human merge, so an agent can't loosen a rule or the allowlist to get a unit through.

`ruff check --config` replaces the repo's own ruff config for that run, so the repo's ignores can't hide a pack rule. `ast-grep` runs with `--no-ignore hidden --globs '!.git'` so a file under a dot directory is scanned and `.git` is not. Both tools must be on `PATH`: `uv tool install ruff ast-grep-cli`.

## The allowlist

The dependency allowlist is the regex in `utils/allowed-package.yml`, one of the three places the pack holds a repo's own data, with `utils/helper-name.yml` and the `ignores` line of `rules/duplicate-helper.yml`; a rerun of the install keeps all three. List the import names of the repo's own top-level packages and of every dependency it allows, the runtime and the dev ones, separated by `|`:

```yaml
rule:
  regex: '^(?:inventory|pytest|yaml)(?:\.|$)'
```

Use the name the code imports (`yaml`), not the one it installs (`PyYAML`). The standard library is always allowed: its public modules (`utils/stdlib-module.yml`, Python 3.12, kept by hand) and any name starting with `_` (`_thread`). Relative imports are never flagged.

`import-linter` is deliberately not used: its contracts only forbid imports and have no allowlist ([D55](../../docs/DESIGN.md)).

## Duplicate helpers

`duplicate-helper` flags functions outside a class body (and classes), never methods. It ships with names the standard library already covers (`mkdir_p` for `os.makedirs(..., exist_ok=True)`, `which` for `shutil.which`, `flatten` for `itertools.chain.from_iterable`). Add the repo's own paved helpers to the regex in `utils/helper-name.yml`, and each helper's home module to `ignores` in `rules/duplicate-helper.yml`, so the helper itself isn't flagged. A duplicate by shape, a new function whose body repeats an existing helper's, needs two definitions compared and stays in review.

## Tests

`tests/run.sh` runs the pack on every example in `tests/examples/<rule>/`: each `fail*.py` must give a finding for that rule, and each `pass*.py` no finding at all. It also runs `tests/allowlist/*.py` against a temp copy of the pack whose `utils/allowed-package.yml` is `inventory`, and, when `cli/target/debug/ns` is built, `ns rules check` with the row on a failing file. A rule directory with a `make.sh` also gets the examples it writes into a temp directory: `file-size`'s long files, and `dependency-allowlist`'s `pass-stdlib-all.py`, which imports every public name in the running Python's `sys.stdlib_module_names` so the hand-kept list can't miss one. A new rule comes with at least one of each. CI and `scripts/ci-local.sh` run it.
