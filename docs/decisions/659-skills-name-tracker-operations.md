---
id: ADR-659
title: "Skills name tracker operations from a closed list, and take each command only from the target repo's docs/agents/issue-tracker.md"
issue: 659
date: 2026-10-14
---

# ADR-659: Skills name tracker operations, never gh or glab commands

## Decision

Agreed with the human on #659 on 2026-10-14. `docs/agents/issue-tracker.md` has an `## Operations` section with one `###` heading per tracker operation (`### Read a PR's state`) and that tracker's command beneath it. The operations are a closed list, `OPERATIONS` in `cli/src/lint/tracker.rs`, and `ns lint` checks that the GitHub and GitLab templates in `skills/ns-setup/references/` give every one a heading and add none outside it.

A skill names the operation in bold and runs the command the target repo's file gives (`skills/ns-contract`, Tracker operations). `ns lint` flags a `gh` or `glab` command, inline or fenced, in a skill's `SKILL.md` or `references/`. It skips the two templates, eval files (fixture tracker docs hold commands on purpose), and `gh auth` or `glab auth`, which are about the tool rather than the tracker.

## Reason

Skills spelled out `gh` commands, some with a GitLab fallback and some without, so a GitLab repo hit `gh` where a skill forgot its fallback: ns-auto's merge detector ran `gh pr view` on every forge. One place per tracker for each command fixes the class, and the lint keeps a new `gh` line from landing in a skill. A closed list keeps the templates in step, so a skill can rely on an operation existing on both forges.
