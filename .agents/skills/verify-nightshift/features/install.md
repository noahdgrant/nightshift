# Install

`ns install` symlinks each `skills/ns-*` directory into the harness skill directories, replaces stale `ns-*` links, and reports a real directory in the way as a conflict without touching it.

## Sub-features

- `install-dry` lists the actions and changes nothing.
- `install-link` creates the links; re-running reports `unchanged`.
- `install-conflict` leaves a real directory alone and exits `1`.

## How to get to it (user POV)

- `ns install [--source <dir>] [--harness <h>]... [--target <dir>]... [--dry-run]`

## Driving it with ns

Preconditions:

- Baseline. `HOME` is `$V/home`, so the targets are under the sandbox. Launch already linked the skills into `$V/home/.claude/skills`, so these recipes use the `agents` harness directory, `$V/home/.agents/skills`, which starts empty.
- `ns install` acts on this repo's skills, so it runs with `--src`. `ns doctor` runs without it, from `$V/repo`.

- **Dry run.** `targets` is `$V/home/.agents/skills`; every action is `link`; `install-dry-none` exits non-zero, because the directory was not created.

  ```bash
  "$V/sandbox" --src cap install-dry ns install --source skills --harness agents --dry-run
  "$V/sandbox" cap install-dry-none test -e "$V/home/.agents/skills"
  ```

- **Link.** `conflicts` is `0`. `install-link-ls` shows links into `$NS_SRC/skills`, and `install-link-doctor` has `.skill_dirs.agents.ns_skills` matching the number of actions.

  ```bash
  "$V/sandbox" --src cap install-link ns install --source skills --harness agents
  "$V/sandbox" cap install-link-ls ls -l "$V/home/.agents/skills"
  "$V/sandbox" cap install-link-doctor ns doctor
  ```

- **Re-run.** Every action is `unchanged`.

  ```bash
  "$V/sandbox" --src cap install-link-again ns install --source skills --harness agents
  ```

- **Conflict.** Replace one link with a real directory, then install. Exit `1`; stdout has `ns-tdd` with `"action": "conflict"` and `"conflicts": 1`; stderr starts `error: 1 conflict(s): real files or unrelated symlinks are in the way`; `install-conflict-dir` exits `0`, so the directory is still there. Then restore the link: `conflicts` is `0` and `install-restore-ls` shows a link again.

  ```bash
  "$V/sandbox" rm "$V/home/.agents/skills/ns-tdd"
  "$V/sandbox" mkdir "$V/home/.agents/skills/ns-tdd"
  "$V/sandbox" --src cap install-conflict ns install --source skills --harness agents
  "$V/sandbox" cap install-conflict-dir test -d "$V/home/.agents/skills/ns-tdd"
  "$V/sandbox" rmdir "$V/home/.agents/skills/ns-tdd"
  "$V/sandbox" --src cap install-restore ns install --source skills --harness agents
  "$V/sandbox" cap install-restore-ls ls -l "$V/home/.agents/skills/ns-tdd"
  ```

## Gotchas

- Outside the sandbox, `HOME` is the operator's, and this links into their real skill directories. Run `ns install` only through `"$V/sandbox"`.
- Without `--source`, `ns install` uses `./skills` if present, else the checkout `ns` was built from.
- Run `ns doctor` without `--src`, as `install-link-doctor` does. From `$NS_SRC` it reads this repo's real `origin` and tries `ssh -T` on its forge host (#536); see [doctor](./doctor.md).
