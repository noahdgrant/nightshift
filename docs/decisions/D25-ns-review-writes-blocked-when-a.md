---
id: D25
title: "ns-review writes blocked when a changed-code Critical stays open"
issue: 110
date: 2026-10-09
amended_by: [D60, ADR-560]
---

# D25: ns-review writes blocked when a changed-code Critical stays open

## Decision

`ns-review` writes `blocked`, not `fail`, when the fix loop stops with a changed-code Critical finding still open. `fail` is left for fix-cycle commits that leave the tests or the verify re-run failing.

## Reason

A `fail` sent the unit back to build, which spent an attempt and usually ended in the same loop (#110). Three fix cycles that could not close a Critical mean a human has to choose: fix it by hand (then delete `review.md` so `ns run` reviews again), split the issue, or send it to `ns-define`. `ns run` already stops on any `blocked` artifact and `ns watch` quotes its first line, so the change is in the skill only. Cargo tests pin the CLI half (blocked stops, fail routes to build); only the model-run eval case `critical-after-three-cycles-is-blocked` checks the skill's rule, and the `fail` half and the multi-cycle loop need multi-cycle model runs tracked with #41. Spec: skills/ns-review/SKILL.md. Amended by D60.
