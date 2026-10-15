# `ns ratchet`

## `ns ratchet check [--factory <dir>] [--json|--human]`

Runs the command of each `[ratchets.<name>]` table in `nightshift.toml` with `sh -c` at the top of the git checkout and compares the number it printed on stdout with the value recorded in `ratchets.json`, next to `nightshift.toml`. A value past the recorded one by more than the tolerance is `worse`; the limit is `recorded + tolerance` when lower is better and `recorded - tolerance` when higher is. A ratchet with no recorded value is `untracked`: reported, never failed, and nothing is recorded. A command that exits non-zero, is killed, runs over 300 s (`NS_RATCHET_TIMEOUT_MS`) or prints anything but one number is `broken`. Each command runs twice, and two values further apart than the tolerance make the ratchet `flaky`.

`--factory <dir>` names the directory holding `nightshift.toml` and `ratchets.json` (default `.nightshift`), and `--json` is the default. `--human` prints one readable line per ratchet instead.

Prints `{ok,command,recorded,ratchets:[{name,command,better,tolerance,recorded,measured,runs,limit,status,error}]}`; `status` is `ok`, `better`, `worse`, `untracked`, `broken` or `flaky`. Exits 0 when none is worse, broken or flaky, 1 when one is, and 2 when not in a git repo or `nightshift.toml` is bad. Never edit `ratchets.json` or a `[ratchets.*]` table to pass: loosening is a human decision.

## `ns ratchet update [--factory <dir>] [--allow-loosen] [--dry-run] [--json|--human]`

For a human or a scheduled job, never a unit. Measures each ratchet twice as `ns ratchet check` does and records the values in `ratchets.json`: an `untracked` ratchet is `recorded`, a better one `tightened`, an equal one `unchanged`. A worse one, even within its tolerance, keeps its recorded value (`kept`) unless `--allow-loosen` records it (`loosened`). Values of ratchets no longer defined are dropped. A broken or flaky ratchet writes nothing, `--dry-run` prints what would change without writing, and a rerun with nothing to change writes nothing.

`--factory <dir>` names the directory holding `nightshift.toml` and `ratchets.json` (default `.nightshift`), and `--json` is the default. `--human` prints one readable line per ratchet instead.

Prints `{ok,command,status,dry_run,recorded,ratchets,dropped}`; `status` is `written`, `planned`, `already-done` or `refused`, and each ratchet's is `recorded`, `tightened`, `unchanged`, `kept`, `loosened`, `broken` or `flaky`. Exits 0 on success, 1 when a ratchet is broken or flaky, and 2 when there is no `[ratchets.*]` table, not in a git repo, or `nightshift.toml` is bad.

See [docs/factory/run.md](../factory/run.md) (Ratchets) for the table, the values file and the gate.
