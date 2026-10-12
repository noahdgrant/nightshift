---
id: D59
title: "ns install --harness links skills into each harness's skill directory"
issue: 51
date: 2026-10-11
---

# D59: ns install --harness links skills into each harness's skill directory

## Decision

`ns install --harness claude|codex|cursor|agents` (#51) links skills into the user-level skill directory each harness documents: `~/.claude/skills` ([Claude Code](https://code.claude.com/docs/en/skills)), `~/.agents/skills` ([Codex](https://developers.openai.com/codex/skills)), `~/.cursor/skills` ([Cursor](https://cursor.com/docs/context/skills)), and `~/.agents/skills` for `agents`. The flag repeats, and its default is `agents` and `claude`. The targets are every `--harness` directory plus every `--target` directory, without repeats; the default applies only when neither flag is given. `ns doctor` reports each harness's directory and its count of `ns-*` skills under `skill_dirs`.

## Reason

The mapping lives in `cli/src/install.rs` next to the doc links. Codex documents `$HOME/.agents/skills` as its user scope, so `codex` and `agents` name one directory. Cursor reads `~/.agents/skills` too, as well as `~/.cursor/skills`; `--harness cursor` still uses Cursor's own directory so the flag means what it says. A union of the two flags is the simplest rule: neither overrides the other, so no flag silently drops a target. `ns watch` still looks for installed skills in the default directories only.
