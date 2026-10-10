# Python rule pack

Install the Python rule pack, so `ns rules check` and the build gate enforce its rules. The pack's README says what each rule checks.

1. **Find the pack.** `ns install` symlinks this skill from the nightshift checkout, and the pack sits beside the skills there: `"$(realpath <this skill's directory>)/../../rules/python"`. If that directory is missing, ask the user for the path to their nightshift checkout.
2. **Check the tools.** `ruff --version` and `ast-grep --version`. A missing one: give the user `uv tool install ruff ast-grep-cli` and wait until both run.
3. **Copy.** Copy `ruff.toml`, `sgconfig.yml`, `file-size.sh`, `rules/` and `utils/` from the pack to `.nightshift/rules/python/` in the target repo. On a rerun, keep the target's allowlist and helper names and take everything else from the pack.
4. **Fill the allowlist.** It is the regex marked `# the allowlist` in `.nightshift/rules/python/rules/dependency-allowlist.yml`. List the import name of every top-level package or module the repo's own code imports (its packages under `src/` or the root, and test helpers such as `conftest`) and of every dependency in `pyproject.toml`: `[project] dependencies`, `[project.optional-dependencies]` and `[dependency-groups]`. Use the import name, which can differ from the distribution name (`PyYAML` is `yaml`): confirm it in the package's metadata (`top_level.txt`) or with the user.
5. **Add the row.** Put this row in the Commands table of `docs/agents/stack.md`, after `typecheck`:

   ```markdown
   | rules | `ruff check --config .nightshift/rules/python/ruff.toml --no-cache --output-format=concise .`, `ast-grep scan --report-style short -c .nightshift/rules/python/sgconfig.yml .`, `.nightshift/rules/python/file-size.sh .` | 1 s |
   ```

6. **Run it.** `ns rules check --human` from the repo root. Show the user the findings. A `dependency-allowlist` finding on a package the repo means to use is a gap in step 4: fix the allowlist and run it again.
7. **Baseline.** The rest are existing debt. Give the user `ns rules baseline --allow-growth` to accept them. It is theirs to run, since it is what lets existing violations pass.

Done when `.nightshift/rules/python/` holds the pack, the allowlist names every package the repo imports on purpose, the row is in `stack.md`, and `ns rules check` exits 0 or the user has the baseline command.
