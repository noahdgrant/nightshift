# Worktrees

`ns worktree` gives each unit its own git worktree on branch `ns/<unit-id>`, with a `.ns/<unit-id>/` artifact folder, lists them with their latest artifact status, and removes them while keeping the branch.

## Sub-features

- `wt-new` creates `<repo>/../<repo>.worktrees/<unit-id>` on `ns/<unit-id>` and prints its JSON.
- `wt-idempotent` prints the same JSON when run again.
- `wt-invalid` rejects a unit id outside `[a-z0-9][a-z0-9-]*` with exit 2.
- `wt-list` lists ns worktrees with `status` from the newest artifact.
- `wt-setup` runs `[worktree] setup` from `.nightshift/nightshift.toml` in a new worktree.
- `wt-remove` removes the worktree and keeps the branch.

## How to get to it (user POV)

- `ns worktree new <unit-id> [--base <ref>] [--no-setup]`
- `ns worktree list`
- `ns worktree setup <unit-id>`
- `ns worktree remove <unit-id> [--dry-run]`

## Driving it with ns

Preconditions:

- Baseline, in `$V/repo`. No `$V/repo.worktrees/` yet.

- **Create.** Run `cap wt-new "$NS" worktree new 7-fix-the-thing`. Exit `0`; `branch` is `ns/7-fix-the-thing` and `path` is `$V/repo.worktrees/7-fix-the-thing`.
- **Idempotent.** Run `cap wt-idempotent "$NS" worktree new 7-fix-the-thing`. Same JSON as `wt-new`.
- **Invalid id.** Run `cap wt-invalid "$NS" worktree new BAD`. Exit `2` and stderr says `invalid unit id "BAD"`.
- **List.** Run `cap wt-list "$NS" worktree list`. One entry for `7-fix-the-thing`, `status` `null` until an artifact exists.
- **Setup.** Write `.nightshift/nightshift.toml` with `[worktree]\nsetup = ["touch made-by-setup"]`, then `cap wt-setup "$NS" worktree new 8-setup`. `setup` lists `{"run":"touch made-by-setup","exit":0}` and `$V/repo.worktrees/8-setup/made-by-setup` exists. Then `rm -r .nightshift`, so later recipes start without a factory definition.
- **Remove.** Run `cap wt-remove-dry "$NS" worktree remove 7-fix-the-thing --dry-run`, then `cap wt-remove "$NS" worktree remove 7-fix-the-thing`. Both print `"branch_kept":true`; afterwards the path is gone and `git -C "$V/repo" branch` still lists `ns/7-fix-the-thing`.

## Gotchas

- `.ns/` is added to `.git/info/exclude`, so artifacts never show in `git status`. Check the folder with `ls`, not `git status`.
- `--repo` and the current directory decide which repo; running from `$NS_SRC` by mistake creates a worktree of nightshift itself.
- A failing setup command exits `1` and leaves the worktree; `ns worktree setup <unit-id>` retries it.
