---
id: ADR-379
title: "Every body nightshift posts opens with a disclosure line"
issue: 379
date: 2026-10-11
amends: [D61]
---

# ADR-379: Every body nightshift posts opens with a disclosure line

## Decision

Every issue body, comment, and PR or merge request body nightshift posts opens with `🤖 *Posted by nightshift, an AI agent.*`, then a blank line, then the text (#379). It replaces the footer `ns watch` appended and the line triage opened its notes with. Titles and commit messages carry no line. An assistant session (not nightshift) opens its posts with `🤖 *Posted by an AI agent (Claude Code).*`, as `AGENTS.md` says.

In the CLI, the forge's `comment`, `comment_pr` and `create_issue` take a `Post`, which only `Post::new` builds, so no forge call can leave the line out. A skill that posts through `gh`, `glab` or a tracker file writes the line itself, per `ns-contract` (Disclosure). `ns watch` reads `Blocked by:` on the line after a leading `🤖` line.

## Reason

One line at the top tells a reader who wrote the text before they read it, the same way on every surface. A footer is missed on a long comment, and three wordings made it unclear what counted as AI text.
