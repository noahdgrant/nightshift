# JavaScript/TypeScript rule pack

Install the JavaScript/TypeScript rule pack, so `ns rules check` and the build gate enforce its rules. The pack's [README](../../../rules/js/README.md) says what each rule checks.

1. **Find the pack.** `ns install` symlinks this skill from the nightshift checkout, and the pack sits beside the skills there: `"$(realpath <this skill's directory>)/../../rules/js"`. If that directory is missing, ask the user for the path to their nightshift checkout.
2. **Check the tools.** `node --version`, `npm --version`, `jq --version` and `ast-grep --version`. A missing one: give the user `sudo apt-get install nodejs npm jq` or `uv tool install ast-grep-cli`, and wait until each runs.
3. **Install.** Run `<pack>/install.sh <repo>`. It copies the pack to `.nightshift/rules/js/`, runs `npm ci` there from the pack's own lockfile, seeds the dependency allowlist from the repo's `package.json`, and adds the `rules` row to the Commands table of `docs/agents/stack.md`. It leaves the repo's own `package.json` and lockfile as they are, whatever its package manager. A rerun refreshes the pack and keeps `utils/allowed-package.yml`, `utils/helper-name.yml` and the `ignores` line of `rules/duplicate-helper.yml`.
4. **Finish the allowlist.** It is the regex in `.nightshift/rules/js/utils/allowed-package.yml`. Add each import prefix the repo resolves itself: TypeScript `paths` aliases (`@/`), and workspace packages a `package.json` elsewhere in the repo names.
5. **Run it.** `ns rules check --human` from the repo root. Show the user the findings. A `dependency-allowlist` finding on a package the repo means to use is a gap in step 4: fix the allowlist and run it again.
6. **Baseline.** The rest are existing debt. Give the user `ns rules baseline --allow-growth` to accept them. It is theirs to run, since it is what lets existing violations pass.

Done when `.nightshift/rules/js/` holds the pack and its `node_modules`, the allowlist names every package the repo imports on purpose, the row is in `stack.md`, and `ns rules check` exits 0 or the user has the baseline command.
