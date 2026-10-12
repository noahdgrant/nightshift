---
id: D62
title: "ns lint lints repo-local skills too"
issue: 334
date: 2026-10-11
---

# D62: ns lint lints repo-local skills too

## Decision

`ns lint` (#334) lints repo-local skills too. A lint root whose parent directory is `.agents` (`.agents/skills`) holds repo-local skills, which need no `ns-` prefix; every other root keeps the rule. Under `skills/`, a directory is linted if it holds a `SKILL.md` or has an `ns-` name, and others are skipped; under a repo-local root every non-hidden directory is a skill, and one without `SKILL.md` reports `missing SKILL.md`. The root is judged by its canonicalized path. A skill's `features/*.md` files, except `features/README.md`, must have exactly the four H2s of the feature entry contract in order: `Sub-features`, `How to get to it (user POV)`, `Driving it with <tool>`, `Gotchas`. Links inside inline code spans are not checked.

## Reason

Deciding by the root, not a frontmatter field, keeps skills under `skills/` held to the prefix whatever they declare. Linting every `SKILL.md` directory makes a stray non-`ns-` skill under `skills/` fail instead of being skipped. The third heading names the project's control tool, so only its prefix is fixed. The feature shape comes from `verify-nightshift/features/README.md` and `ns-setup-verify`'s template.
