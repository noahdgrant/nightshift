---
id: D19
title: "ns-define routes by size"
date: 2026-10-08
---

# D19: ns-define routes by size

## Decision

`ns-define` routes by size: a grilled Agent Brief for small work, a spec for a feature, and `ns-requirements` then `ns-design-doc` for a new product or subsystem. `ns-verify-docs` verifies doc units. `ns-verify` routes a unit there by a path rule read from the base's `docs/agents/docs.md`, so a diff can't widen its own definition of docs-only. Requirement IDs thread through design, tickets, tests and `evidence.md`. Approving requirements or a design is human-only.

## Reason

Locations, review surface and templates come from the target repo (`docs/agents/docs.md`, overridable templates), so a team's house format stays in its own repos.
