# Conventions

How skills in this repo are written, and how phases hand work to each other. Read this before writing or editing a skill, along with `skills/sf-writing-for-agents`.

## Skill layout

One flat directory per skill: `skills/sf-<name>/SKILL.md`. The directory name and the frontmatter `name` match. Material only some branches need goes in `references/` beside `SKILL.md`, behind a pointer. Scripts go in `scripts/`.

```yaml
---
name: sf-tdd
description: Test-driven development with red-green-refactor. Use when building a feature or fixing a bug test-first.
metadata:
  upstream: mattpocock/skills@b0618bc436ad:skills/engineering/tdd
---
```

- `metadata.upstream` is `<owner>/<repo>@<sha>:<path>`. Set it on every adapted skill, and list several on separate lines (as a YAML list) when a skill merges sources. Omit it on originals. `sf-maintain` diffs upstream from that sha.
- **Principles** (`sf-principle-*`) set `disable-model-invocation: true`. Other skills point to them by relative path, e.g. `[prove it works](../sf-principle-prove-it-works/SKILL.md)`, naming when the principle applies.
- Model-invoked by default: phase skills call leaf skills, and `sf-auto` calls phase skills, so both need a description the agent can match. Set `disable-model-invocation: true` only on skills a human should be the only one to start.

## Harness neutrality

Skills run under any harness that reads `SKILL.md` (Claude Code, Codex, Cursor, OpenCode, ...).

- To use another skill, write "Load the `sf-tdd` skill". Never write a harness's tool name (`Skill`, `Task`, `Agent`).
- To delegate, write "Hand this to a **fresh-context agent**". That means a subagent if the harness has them, otherwise a headless run through `sf ask --role <role>`. If neither is available, do the work inline, and say in the artifact that it ran inline.
- Paths that differ per harness (transcripts, settings) live in the `sf` CLI, not in skills.

## Runtime contract

Units, worktrees, artifacts, gates, evidence and the `docs/agents/` files are defined in [`skills/sf-contract/SKILL.md`](../skills/sf-contract/SKILL.md). It ships with the skills, so installed skills can read it. From a skill, link it as `../sf-contract/SKILL.md`. Never link to `docs/` from a skill: `docs/` isn't installed.

## Prose

- Short, plain sentences. No em dashes.
- Positive instructions. Use a prohibition only as a hard guardrail, and pair it with the target behaviour.
- One meaning in one place. Point to it instead of restating it.
