---
id: ADR-358
title: "Claude phases run a per-unit skill snapshot and the factory's own settings"
issue: 358
date: 2026-10-11
amends: [ADR-354]
---

# ADR-358: Claude phases run a per-unit skill snapshot and the factory's own settings

## Decision

`ns run` pins a unit's skills to the commit the unit forked from (#358, agreed in #352). At unit start it writes that commit's `skills/` and `.agents/skills/` as real files into `.ns/<unit>/skills/`, once per unit, and links the worktree's `.claude/skills` and `.agents/skills` to it, both git-excluded. A repo whose `skills/` tree has no `ns-*` skill gets the installed `ns-*` skills copied instead, and a unit with none to pin is an error. Claude phases, and `ns ask` started inside one, run with `--setting-sources project,local --settings <git-common-dir>/ns/units/<unit>/phase-settings.json --add-dir .ns/<unit>/skills`, so the operator's user settings and personal skills never reach a phase. The settings file is versioned as `.nightshift/phase-settings.json`, built into `ns`, and holds the denies for unattended phases (`sudo`, `terraform apply`, `.env` and `secrets/` reads) and the git hook guard of ADR-354, which moves there from an inline `--settings` value. Codex phases are only partly pinned, a documented gap: they read the snapshot through `.agents/skills` but the user's installed skills still load. `ns doctor` reports the mode per harness.

## Reason

Phases loaded the installed skills, symlinks into the main checkout that `ns watch` fast-forwards before each unit, and Claude Code reloads an edited skill mid-session, so a running unit's skills changed under it, and a personal skill with the same name hid the project's. Pinning to the base commit gives every unit a reproducible skill set that the unit's own skill edits don't change. The operator's deny rules applied to phases even under `bypassPermissions` and blocked a verify sandbox twice (#399); `--setting-sources` drops them, and the factory's file puts back the denies an unattended phase needs. The file lives in `.nightshift/` so a change to what phases may do needs a human merge, and it is built into the binary so every repo `ns` drives gets it. Claude Code's permission rules can't except a directory from a deny, so the `.env` and `secrets/` reads are denied everywhere rather than only outside the worktree. Copying the installed skills for a repo without `skills/` keeps consumers working; their pin is the install at unit start, not a commit.

## Consequences

- The settings file sits under the git common dir, outside every worktree, so a phase cannot edit it; the snapshot stays in `.ns/<unit>/` where the worktree's symlinks reach it, and `ns guard-hooks` guards it on a best-effort basis (the unit's `.ns/<unit>` directory, its `skills`, and links into it).
- `--settings` is a file path now, not inline JSON, and a harness command that carries `--settings` or `--setting-sources` is a usage error, as is `ns ask` in a phase with such a command, or with its `NS_PHASE_SETTINGS` file gone. Remedy: drop the flag from the user config's `command_write`.
- The operator's user-level settings no longer reach claude phases: `env`, hooks, MCP servers, `apiKeyHelper` and their own deny rules. Remedy: move the setting into the project's `.claude/settings.json` or `.claude/settings.local.json`, or into `.nightshift/phase-settings.json` (a human merge).
- The `.env` and `secrets/` denies bind the Read tool only (`cat .env` in Bash passes), so they are not a secrets boundary, and they apply inside the unit's worktree too, since Claude Code cannot except a directory from a deny. `ns ask`'s settings depend on `NS_UNIT`, `NS_WORKTREE` and `NS_PHASE_SETTINGS`, which the phase sets, so they are cooperative.
- A tracked `.claude/skills` that is not a snapshot link is an error at `ns run` start, because claude reads that path.
- `ns doctor` reports `snapshot` for claude, `partial` for codex and cursor-agent, and `none` for gemini and opencode.
