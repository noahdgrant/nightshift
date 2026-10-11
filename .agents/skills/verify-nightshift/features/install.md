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

- Baseline, in `$NS_SRC`. `HOME` is `$V/home`, so the targets are under the sandbox.

- **Dry run.** Run `cap install-dry "$NS" install --source "$NS_SRC/skills" --dry-run`. `targets` are `$V/home/.agents/skills` and `$V/home/.claude/skills`; every action is `link`; `ls $V/home/.claude/skills` is still empty or missing.
- **Link.** Run `cap install-link "$NS" install --source "$NS_SRC/skills" --harness claude`. `conflicts` is `0`. `ls -l $V/home/.claude/skills` shows links into `$NS_SRC/skills`, and `cap install-link-doctor "$NS" doctor` has `.skill_dirs.claude.ns_skills` matching the count.
- **Re-run.** Run the same command again as `cap install-link-again "$NS" install --source "$NS_SRC/skills" --harness claude`. Every action is `unchanged`.
- **Conflict.** Run `rm $V/home/.claude/skills/ns-tdd && mkdir $V/home/.claude/skills/ns-tdd` (plain setup), then `cap install-conflict "$NS" install --source "$NS_SRC/skills" --harness claude`. Exit `1`, stderr names the conflict, and the directory is still there.

## Gotchas

- Without `HOME=$V/home` this links into the operator's real skill directories. Doctor's check of `HOME` matters here more than anywhere.
- Without `--source`, `ns install` uses `./skills` if present, else the checkout `ns` was built from.
