# `ns worktree`

## `ns worktree new <unit-id> [--base <branch>] [--repo <path>] [--no-setup]`

Creates `<root>/../<repo-name>.worktrees/<unit-id>` on branch `ns/<unit-id>`, where `<root>` is the main worktree of the repo you run it from. The base defaults to the local branch behind `origin/HEAD`, else the current `HEAD`. It also creates `.ns/<unit-id>/` in the worktree and adds `.ns/` to the common `.git/info/exclude`.

Prints `{"unit","path","branch","artifacts","setup"}`. If the worktree exists, it prints the same JSON. If only the branch exists (after `remove`), it checks the branch out again. Unit ids match `[a-z0-9][a-z0-9-]*`.

When the worktree is newly created, `new` runs the setup commands from `<root>/.nightshift/nightshift.toml`:

```toml
[worktree]
setup = ["git submodule update --init"]
```

Each command runs with `sh -c` in the new worktree, with `NS_UNIT`, `NS_WORKTREE` and `NS_MAIN_ROOT` set. Their output goes to stderr. `setup` in the JSON lists `{"run","exit"}` per command that ran. The first failing command stops the rest and makes `new` exit 1. The worktree stays in place, and `ns worktree setup <unit-id>` retries. An idempotent repeat of `new` runs nothing and prints `"setup": []`, and so does `--no-setup`.

`.nightshift/` is the factory definition root (D15 in `docs/decisions/`). The whole file is parsed against the schema in [docs/FACTORY.md](../FACTORY.md), and unknown tables or keys are rejected (exit 2).

## `ns worktree setup <unit-id> [--repo <path>]`

Re-runs the `[worktree] setup` commands in the unit's existing worktree. Prints `{"unit","path","setup"}` and exits 1 if a command fails. A unit with no worktree is an error that names `ns worktree new`.

## `ns worktree list [--repo <path>]`

Prints a JSON array of worktrees on `ns/*` branches. Each entry has `unit`, `path`, `branch`, `artifacts` and `status`. `status` is `null`, or `{"file","phase","status","updated"}` from the artifact with the newest `updated` in its frontmatter. Ties go to the later phase.

## `ns worktree remove <unit-id> [--dry-run] [--force] [--repo <path>]`

Removes the unit's worktree and keeps its branch. A worktree with uncommitted changes is refused unless `--force` is given. Artifacts under `.ns/` are ignored, so they don't count as changes. Removing a unit that has no worktree prints `"action": "none"` and exits 0.
