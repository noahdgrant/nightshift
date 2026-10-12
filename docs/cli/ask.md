# `ns ask`

## `ns ask --role <role> [--prompt-file <f>] [--write] [--cwd <dir>] [--dry-run]`

Runs the harness configured for the role. The prompt is read from `--prompt-file`, or from stdin, and sent to the harness on stdin. The harness's stdout streams through. The default is read-only. `--write` uses the harness's `command_write`, so the harness can edit files and run commands. `--cwd` runs the harness in that directory, such as a unit's worktree. `--dry-run` prints the resolved command, mode, cwd and whether the binary is on PATH, as JSON, without running anything.

The role lookup falls back from the most specific name: `review.security`, then `review`, then `default`.

Under the factory's `billing = "subscription"` (the default, and the mode outside a repo), the harness never sees the API-key variables of claude or codex. See Billing in `docs/FACTORY.md`.
