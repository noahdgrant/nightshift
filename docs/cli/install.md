# `ns install`

## `ns install [--source <dir>] [--harness <h>]... [--target <dir>]... [--dry-run]`

Symlinks each `<source>/ns-*` directory (default source `./skills`) into each target. `--harness` picks a harness's skill directory, and may repeat:

| Harness | Directory |
|---|---|
| `claude` | `~/.claude/skills` |
| `codex` | `~/.agents/skills` |
| `agents` | `~/.agents/skills` |
| `cursor` | `~/.cursor/skills` |

The targets are every `--harness` directory plus every `--target` directory, with duplicates removed. With neither flag, the default is `--harness agents --harness claude`. Target directories are created if missing. Each action is one of:

| Action | Meaning |
|---|---|
| `link` | new symlink |
| `unchanged` | already points at the source |
| `relink` | replaced an `ns-*` symlink that pointed at another checkout of the same skill, or at nothing |
| `prune` | removed a dangling `ns-*` symlink into the source, for a skill that was deleted |
| `conflict` | a real directory or an unrelated symlink is in the way. Left alone, and the command exits 1 |
