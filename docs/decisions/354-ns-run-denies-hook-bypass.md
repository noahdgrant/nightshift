---
id: ADR-354
title: "ns run denies hook bypasses in claude phases"
issue: 354
date: 2026-10-11
---

# ADR-354: ns run denies hook bypasses in claude phases

## Decision

`ns run` enforces `ns-contract`'s Git hooks rule for claude phases (#354, settled in #350). Each claude phase gets `--settings` with one `PreToolUse` hook on `Bash`, `Edit`, `Write`, `MultiEdit` and `NotebookEdit` that runs the hidden `ns guard-hooks`. It tokenises the Bash command, so a quoted mention or a heredoc body is not a command, and denies `--no-verify`/`-n` on `git commit`, `--no-verify` on push, merge and rebase, a `core.hooksPath` override or write, a write, move or delete under `.githooks/`, and `HUSKY=0`, `SKIP=` and the `GIT_CONFIG_*` variables, naming the rule. The user's own settings are untouched. Other harnesses have no such hook; the gate run after every phase that moves HEAD (#351) is their backstop.

## Reason

A shell parser that covers every form (an alias, a script the phase writes and runs, `cd .githooks && rm *`) is out of reach. The hook stops the plain forms, and the gate catches what slips past.
