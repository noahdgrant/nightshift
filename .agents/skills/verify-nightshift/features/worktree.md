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

- Baseline. No `$V/repo.worktrees/` yet.

- **Create.** Exit `0`; `branch` is `ns/7-fix-the-thing` and `path` is `$V/repo.worktrees/7-fix-the-thing`.

  ```bash
  "$V/sandbox" cap wt-new ns worktree new 7-fix-the-thing
  ```

- **Idempotent.** Same JSON as `wt-new`.

  ```bash
  "$V/sandbox" cap wt-idempotent ns worktree new 7-fix-the-thing
  ```

- **Invalid id.** Exit `2` and stderr says `invalid unit id "BAD"`.

  ```bash
  "$V/sandbox" cap wt-invalid ns worktree new BAD
  ```

- **List.** One entry for `7-fix-the-thing`, `status` `null` until an artifact exists.

  ```bash
  "$V/sandbox" cap wt-list ns worktree list
  ```

- **Setup.** Write a factory file whose `[worktree] setup` touches a file, then create a worktree. `setup` lists `{"run":"touch made-by-setup","exit":0}` and `wt-setup-file` exits `0`. The last line removes `.nightshift/`, so later recipes start without a factory definition.

  ```bash
  "$V/sandbox" sh -c 'mkdir -p .nightshift && printf "$1" > .nightshift/nightshift.toml' _ '[worktree]\nsetup = ["touch made-by-setup"]\n'
  "$V/sandbox" cap wt-setup ns worktree new 8-setup
  "$V/sandbox" cap wt-setup-file test -f "$V/repo.worktrees/8-setup/made-by-setup"
  "$V/sandbox" rm -r "$V/repo/.nightshift"
  ```

- **Remove.** Both print `"branch_kept":true`; afterwards `wt-remove-gone` exits non-zero (the path is gone) and `wt-remove-branch` still lists `ns/7-fix-the-thing`.

  ```bash
  "$V/sandbox" cap wt-remove-dry ns worktree remove 7-fix-the-thing --dry-run
  "$V/sandbox" cap wt-remove ns worktree remove 7-fix-the-thing
  "$V/sandbox" cap wt-remove-gone test -e "$V/repo.worktrees/7-fix-the-thing"
  "$V/sandbox" cap wt-remove-branch git branch
  ```

## Gotchas

- `.ns/` is added to `.git/info/exclude`, so artifacts never show in `git status`. Check the folder with `ls`, not `git status`.
- `--repo` and the current directory decide which repo. `"$V/sandbox"` runs in `$V/repo`; `"$V/sandbox" --src` would create a worktree of nightshift itself.
- A failing setup command exits `1` and leaves the worktree; `ns worktree setup <unit-id>` retries it.
