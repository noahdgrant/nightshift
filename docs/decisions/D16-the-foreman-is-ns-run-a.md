---
id: D16
title: "The foreman is ns run, a deterministic state machine in the CLI, not an agent"
date: 2026-10-08
---

# D16: The foreman is ns run, a deterministic state machine in the CLI, not an agent

## Decision

The foreman is `ns run`, a deterministic state machine in the CLI, not an agent. It reads artifact frontmatter, starts the agent the definition assigns to the next phase, and enforces gates, retries, bench locks and the human-only actions in code.

## Reason

Agents keep the judgement calls (triage, review). `ns-auto` becomes the in-session form of the same loop.
