# Python rule pack

The rules `ns rules check` runs on a Python repo. They compose two existing tools, `ruff` and `ast-grep`, and each one enforces a principle or quality-bar item. Rules no tool can express stay in review.

| Rule | Tool | Fails on | Enforces |
|---|---|---|---|
| `PGH003` | ruff | `# type: ignore` with no error code | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no suppression hides a real bug |
| `PGH004` | ruff | `# noqa` with no rule code | the same |
| `suppression-reason` | ast-grep | a suppression (`noqa`, `type: ignore`, `pyright: ignore`, `pylint: disable`, `ruff: noqa`) with no reason after it on the same line | the same |
| `dependency-allowlist` | ast-grep | an import from outside the standard library and the allowlist | [pave the road](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-principle-pave-the-road/SKILL.md): one road per job |
| `duplicate-helper` | ast-grep | a function or class named like a helper that already exists | the same |
| `file-size` | ast-grep | a file with content past line 1000 | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no file grows past roughly 1000 lines |

A suppression's reason is a second comment after it:

```python
import os  # noqa: F401  # re-exported for callers
```

## Install

The `ns-setup` skill installs the pack. By hand:

1. Copy `ruff.toml`, `sgconfig.yml`, `rules/` and `utils/` to `.nightshift/rules/python/` in the target repo. `.nightshift/**` needs a human merge, so an agent can't loosen a rule or the allowlist to get a unit through.
2. Fill in the allowlist (below).
3. Add the `rules` row to the Commands table in `docs/agents/stack.md`:

   ```markdown
   | rules | `ruff check --config .nightshift/rules/python/ruff.toml --no-cache --output-format=concise .`, `ast-grep scan --report-style short -c .nightshift/rules/python/sgconfig.yml .` | 1 s |
   ```

4. Run `ns rules check` to see the findings, then have a human accept the existing ones with `ns rules baseline --allow-growth`.

`ruff check --config` replaces the repo's own ruff config for that run, so the repo's ignores can't hide a pack rule. Both tools must be on `PATH`: `uv tool install ruff` and `uv tool install ast-grep-cli`.

## The allowlist

The dependency allowlist is the regex marked `# the allowlist` in `rules/dependency-allowlist.yml`. List the import names of the repo's own top-level packages and of every dependency it allows, the runtime and the dev ones, separated by `|`:

```yaml
      - regex: '^(?:inventory|pytest|yaml)(?:\.|$)' # the allowlist
```

Use the name the code imports (`yaml`), not the one it installs (`PyYAML`). The standard library is always allowed (`utils/stdlib-module.yml`). Relative imports are never flagged.

## Duplicate helpers

`duplicate-helper` ships with names the standard library already covers (`mkdir_p` for `os.makedirs(..., exist_ok=True)`, `which` for `shutil.which`, `flatten` for `itertools.chain.from_iterable`). Add the repo's own paved helpers to its regex, and each helper's home module to `ignores`, so the helper itself isn't flagged. A duplicate by shape, a new function whose body repeats an existing helper's, needs two definitions compared and stays in review.

## Tests

`tests/run.sh` runs both tools on every example in `tests/examples/<rule>/`: each `fail*.py` must give a finding for that rule, and each `pass*.py` no finding at all. A new rule comes with at least one of each. CI and `scripts/ci-local.sh` run it.
