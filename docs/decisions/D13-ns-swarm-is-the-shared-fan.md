---
id: D13
title: "ns-swarm is the shared fan-out leaf"
date: 2026-10-08
---

# D13: ns-swarm is the shared fan-out leaf

## Decision

`ns-swarm` is the shared fan-out leaf. `ns-review` (split by axis) and `ns-verify` (split by feature) use it.

## Reason

One source of truth for fan-out, aggregation and the PASS/ISSUES/BLOCKED report.
