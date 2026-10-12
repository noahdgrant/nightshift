---
id: D47
title: "An agent-readiness audit, ns-agent-readiness, finds what in a codebase helps or hurts agents"
issue: 214
date: 2026-10-10
---

# D47: An agent-readiness audit, ns-agent-readiness, finds what in a codebase helps or hurts agents

## Decision

An agent-readiness audit, `ns-agent-readiness`, finds what in a codebase helps or hurts agents. It scans git-history hot spots, not the whole tree, with five check groups: one road per job, road is the shortcut, well-trodden stones (the three from `ns-principle-pave-the-road`), navigability (files past about 1000 lines, shallow modules by the deletion test) and the verify loop. It writes a ranked markdown report to `.ns/agent-readiness/<timestamp>/report.md`; each finding names its files, what an agent would copy or get wrong, the fix at the highest rung of the trust ladder that works, and a strength (Strong, Worth exploring, Speculative). Under `gates: auto` it files the top Strong and Worth exploring findings as `needs-triage` issues with category and area labels, skipping duplicates, up to `audit issues per run: <n>` in `docs/agents/issue-tracker.md` (5 when absent); under `gates: stop` the human picks. `ns-setup` writes that line and offers the audit at the end of onboarding, under `gates: stop`.

## Reason

Onboarding wrote `docs/agents/` but never asked whether the code itself leads agents wrong (#214). Agents copy what is nearby, so a second road or a comment-only rule turns into the next bug; the report says what an agent would get wrong so a human can weigh it. Adapted from mattpocock/skills `improve-codebase-architecture` and `codebase-design`: their hot-spot scope (a finding in a file nobody touches costs agents nothing) and deep-module vocabulary, with the pave-the-road groups as the lens. Markdown, not the upstream's HTML: it lives beside the other `.ns/` artifacts and reads in a terminal or a PR. Speculative findings stay in the report so an unattended run doesn't flood triage with guesses, and the cap limits floods where issues are created, as D42 expects. Onboarding runs it under `gates: stop` because a human is present and filing is their call. The scheduled weekly run is #231 and #38. Evals: trigger cases and `py-two-config-loaders` (two config loaders and a comment-only rule, cap 1).
