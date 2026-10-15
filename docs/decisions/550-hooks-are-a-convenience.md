---
id: ADR-550
title: "Hooks are a convenience: ns run puts back tampered protected paths, and the guard checks only literal forms"
issue: 550
date: 2026-10-15
amends: [ADR-354]
---

# ADR-550: Hooks are a convenience: ns run puts back tampered protected paths, and the guard checks only literal forms

## Decision

Correctness never depends on a repo's git hooks. `ns run`'s gates run outside the agent, after the phase, and required CI plus the merge step decide what reaches the default branch. A skipped local hook can't land unchecked code. The hooks give a phase fast feedback; the gates and CI are the authority.

`ns run` detects tampering instead of predicting it. Before each phase's harness starts, it reads every protected path: the common `.git/config` and any `config.worktree`, `.git/hooks/`, the worktree's `.githooks/`, the unit's `.ns/<unit>/skills/` and phase settings file, and each skills link, with the directory it points at when that is not the snapshot. After the harness ends, however it ended (passed, failed, timed out, killed or stopped), it reads them again. A path that changed, appeared or went away is put back from the record. The phase is recorded as `tampered`, what it wrote is set aside as `<artifact>-tampered`, and the unit ends stuck with a reason that names the paths. A git config file is compared by its entries without the `branch.*` keys, because `git push -u` and other units' pushes set upstreams in the shared config; every other path is compared by its bytes, mode and link target.

`ns guard-hooks` shrinks to literal checks, for an early, helpful denial only: `--no-verify` on `git commit` and `git push`, `-n` on commit, `-c core.hooksPath` (and its `--config-env` form), `--git-dir`, `GIT_DIR=`, and a write, move or delete of a protected path named literally. It splits a command into words, honouring quotes and skipping comments and heredoc bodies, and follows no variable, alias, nested shell or working directory. This amends ADR-354, whose guard read through expansions, wrappers, `eval`, `sh -c` and `cd`.

## Reason

A denylist over Bash can't be complete. On 2026-10-12 and 13, 15 of 22 review escapes were guard holes (`alias`, `hash`, `PATH`, `pushd`, `$OLDPWD`, `BASH_ENV`, process substitution, `trap`, lexer edge cases), and every guard change brought more. Reading the files after the phase needs no shell parsing: whatever wrote a protected path, the change is seen and undone. The check reads a few hundred small files, well under a second a phase.

Ending the unit stuck, rather than retrying the phase, puts a human in front of a phase that worked against the factory's rules. Ignoring `branch.*` keys in the config is safe because none of them moves or skips a hook. When a phase did tamper, the config goes back to its bytes from before the phase, so an upstream set during that phase is lost too; the phase failed anyway. The guard's remaining literal checks still catch the common, lazy forms before they run, which saves a phase the cost of a stuck unit.
