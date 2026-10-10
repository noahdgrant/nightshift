# Python rule pack

The rules `ns rules check` runs on a Python repo. They compose two existing tools, `ruff` and `ast-grep`, and each one enforces a principle or quality-bar item. Rules no tool can express stay in review.

| Rule | Tool | Fails on | Enforces |
|---|---|---|---|
| `PGH003` | ruff | `# type: ignore` with no error code | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no suppression hides a real bug |
| `PGH004` | ruff | `# noqa` with no rule code | the same |
| `suppression-reason` | ast-grep | a suppression (`noqa`, `type: ignore`, `pyright: ignore`, `pylint: disable`, `ruff: noqa`) with no reason after it on the same line | the same |
| `dependency-allowlist` | ast-grep | an import from outside the standard library and the allowlist | [pave the road](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-principle-pave-the-road/SKILL.md): one road per job |
| `duplicate-helper` | ast-grep | a function or class named like a helper that already exists | the same |
| `file-size` | `file-size.sh` | a file whose last non-blank line is past line 1000, blank lines at the top included | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no file grows past roughly 1000 lines |

A suppression's reason is a second comment after it:

```python
import os  # noqa: F401  # re-exported for callers
```

The regex only checks that a second comment follows the suppression, so any text there counts as its reason. Whether the reason is a good one stays in review.

## Install

The `ns-setup` skill installs the pack, and [its steps](../../skills/ns-setup/references/rules-python.md) work by hand too. It copies `ruff.toml`, `sgconfig.yml`, `file-size.sh`, `rules/` and `utils/` to `.nightshift/rules/python/` in the target repo and adds a `rules` row to `docs/agents/stack.md`. `.nightshift/**` needs a human merge, so an agent can't loosen a rule or the allowlist to get a unit through.

`ruff check --config` replaces the repo's own ruff config for that run, so the repo's ignores can't hide a pack rule. Both tools must be on `PATH`: `uv tool install ruff ast-grep-cli`.

## The allowlist

The dependency allowlist is the regex marked `# the allowlist` in `rules/dependency-allowlist.yml`. List the import names of the repo's own top-level packages and of every dependency it allows, the runtime and the dev ones, separated by `|`:

```yaml
      - regex: '^(?:inventory|pytest|yaml)(?:\.|$)' # the allowlist
```

Use the name the code imports (`yaml`), not the one it installs (`PyYAML`). The standard library is always allowed: its public modules (`utils/stdlib-module.yml`, Python 3.12, kept by hand) and any name starting with `_` (`_thread`). Relative imports are never flagged.

`import-linter` is deliberately not used: its contracts only forbid imports and have no allowlist ([D52](../../docs/DESIGN.md)).

## Duplicate helpers

`duplicate-helper` flags functions outside a class body (and classes), never methods. It ships with names the standard library already covers (`mkdir_p` for `os.makedirs(..., exist_ok=True)`, `which` for `shutil.which`, `flatten` for `itertools.chain.from_iterable`). Add the repo's own paved helpers to its regex, and each helper's home module to `ignores`, so the helper itself isn't flagged. A duplicate by shape, a new function whose body repeats an existing helper's, needs two definitions compared and stays in review.

## Tests

`tests/run.sh` runs the pack on every example in `tests/examples/<rule>/`: each `fail*.py` must give a finding for that rule, and each `pass*.py` no finding at all. It also runs `tests/allowlist/*.py` against a temp copy of the pack whose allowlist is `inventory`, found by the `# the allowlist` marker, which must appear exactly once. A rule directory with a `make.sh` also gets the examples it writes into a temp directory: `file-size`'s long files, and `dependency-allowlist`'s `pass-stdlib-all.py`, which imports every public name in the running Python's `sys.stdlib_module_names` so the hand-kept list can't miss one. A new rule comes with at least one of each. CI and `scripts/ci-local.sh` run it.
