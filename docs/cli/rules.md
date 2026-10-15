# `ns rules`

## `ns rules check [--human] [--only <text>] [--prune]`

Runs the repo's rule pack and fails only on the violations the committed baseline doesn't list, so existing code never blocks a unit. It runs each command in the `rules` row of `docs/agents/stack.md` with `sh -c` at the top of the git checkout, reads every line that is a finding, and matches it against `.nightshift/rules-baseline.json` on `(tool, rule, file, snippet)`, so a moved line still matches. A command that exits non-zero and prints no finding, is killed, exits 126 or 127, runs over 300 s (`NS_RULES_TIMEOUT_MS`) or prints over 8 MiB (`NS_RULES_MAX_OUTPUT_BYTES`) is an error. With no `rules` row it runs nothing and exits 0.

`--only <text>` runs only the commands that contain `<text>`, and is a usage error when the row has commands but none contains it; its `fixed` is always empty, since the entries of the commands it skipped would read as fixed. `--prune`, when no violation is new, rewrites the baseline without its fixed entries; it never adds one, and with `--only` it writes nothing. The gate after build runs it ([docs/factory/run.md](../factory/run.md), Rules).

Prints `{ok,command,baseline,commands,findings,baselined,new,fixed,pruned}`; `fixed` lists baseline entries nothing matched, and `pruned` how many `--prune` dropped. Exits 0 when nothing is new, 1 when something is new or a command failed, and 2 when not in a git repo or `--only` matched no command. `--human` prints one readable line per new or fixed violation instead of JSON.

## `ns rules baseline [--allow-growth] [--dry-run] [--human]`

Runs the rule pack as `ns rules check` does and writes every current violation to `.nightshift/rules-baseline.json`, dropping the entries that are gone. A violation the baseline lacks, every one when there is no baseline yet, is growth: without `--allow-growth` it writes nothing and exits 1. A rerun with nothing added or dropped writes nothing, and `--dry-run` prints what would change without writing. `--allow-growth` is a human accepting debt, never an agent.

Prints `{ok,command,status,dry_run,baseline,commands,violations,added,dropped}`; `status` is `written`, `planned`, `already-done` or `refused`. `--human` prints one readable line per added or dropped violation instead of JSON. Exits 0 on success, 1 when growth is refused or a command failed, and 2 when there is no `rules` row or it is not in a git repo.

See [docs/factory/run.md](../factory/run.md) (Rules) for the rule pack, the finding formats and the baseline's matching.
