# `ns clean`

## `ns clean [--dry-run]`

Removes every unit worktree, and its local `ns/<unit>` branch, whose issue is closed and whose PR (from `pr.md`) merged, closes the issue and comes from `ns/<unit>`. It saves the units' quality records first; records left in the outbox count as saved. It never removes a worktree with uncommitted changes to tracked files, one whose HEAD left `ns/<unit>`, one whose branch holds changes the merged PR head doesn't (whitespace counts), or a unit whose run lock a live `ns run` holds. Prints `{ok, command, dry_run, status, removed, kept, records}`, with a reason for every kept unit; `status` is `cleaned`, `planned` or `nothing-to-clean`. `--dry-run` prints `planned` and removes or records nothing, though it reads `gh` and may fetch a PR head. Exits 1 when a removal failed. Details in `docs/factory/unit-lifecycle.md` (Cleanup).
