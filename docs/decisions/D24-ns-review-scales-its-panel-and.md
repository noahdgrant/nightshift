---
id: D24
title: "ns-review scales its panel and mutation runs with the change"
issue: 109
date: 2026-10-09
amended_by: [D60]
---

# D24: ns-review scales its panel and mutation runs with the change

## Decision

`ns-review` scales its panel and its mutation runs with the change, and a fix cycle re-asks only the axes that still have findings, plus correctness. Every open Critical and Important finding is still fixed on every axis within the 3-cycle limit.

## Reason

Review took 49% of overnight phase time (#109): eight reviewers on every pass, and up to five full-suite runs per pass. The saving comes from not re-asking axes that already passed. Deliberate choices: a diff under `skills/`, `AGENTS.md`, `CLAUDE.md` or `docs/agents/` is agent-facing instructions and gets the full panel; a reviewer whose findings were all dismissed is not re-run; each mutation gets a timeout of three times the listed test time (at least 60 seconds) and an inconclusive one reruns once at double. No eval case yet checks that a large diff still gets the full panel (it needs a fixture), and the fix-loop, later-pass and timeout rules need multi-cycle model runs; both are tracked with #41. Spec: skills/ns-review/SKILL.md. Amended by D60.
