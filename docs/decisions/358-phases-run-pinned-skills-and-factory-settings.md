---
id: ADR-358
title: "Claude phases run a per-unit skill snapshot and the factory's own settings"
issue: 358
date: 2026-10-11
amends: [ADR-354]
---

# ADR-358: Claude phases run a per-unit skill snapshot and the factory's own settings

## Decision

`ns run` pins a unit's skills to the commit the unit forked from (#358, agreed in #352). At unit start it writes that commit's `skills/` and `.agents/skills/` as real files into `.ns/<unit>/skills/`, once per unit, and links the worktree's `.claude/skills` and `.agents/skills` to it, both git-excluded. A repo whose `skills/` tree has no `ns-*` skill gets the installed `ns-*` skills copied instead, and a unit with none to pin is an error. Claude phases, and `ns ask` started inside one, run with `--setting-sources project,local --settings .ns/<unit>/phase-settings.json`, so the operator's user settings and personal skills never reach a phase. The settings file is versioned as `.nightshift/phase-settings.json`, built into `ns`, and holds the denies for unattended phases (`sudo`, `terraform apply`, `.env` and `secrets/` reads) and the git hook guard of ADR-354, which moves there from an inline `--settings` value. Codex phases stay unpinned, a documented gap. `ns doctor` reports the mode per harness.

## Reason

Phases loaded the installed skills, symlinks into the main checkout that `ns watch` fast-forwards before each unit, and Claude Code reloads an edited skill mid-session, so a running unit's skills changed under it, and a personal skill with the same name hid the project's. Pinning to the base commit gives every unit a reproducible skill set that the unit's own skill edits don't change. The operator's deny rules applied to phases even under `bypassPermissions` and blocked a verify sandbox twice (#399); `--setting-sources` drops them, and the factory's file puts back the denies an unattended phase needs. The file lives in `.nightshift/` so a change to what phases may do needs a human merge, and it is built into the binary so every repo `ns` drives gets it. Claude Code's permission rules can't except a directory from a deny, so the `.env` and `secrets/` reads are denied everywhere rather than only outside the worktree. Copying the installed skills for a repo without `skills/` keeps consumers working; their pin is the install at unit start, not a commit.
