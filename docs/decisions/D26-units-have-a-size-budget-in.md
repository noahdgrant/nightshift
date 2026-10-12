---
id: D26
title: "Units have a size budget in changed lines"
issue: 116
date: 2026-10-09
amended_by: [D34, D35, D37]
---

# D26: Units have a size budget in changed lines

## Decision

Units have a size budget in changed lines, set in `docs/agents/stack.md` as `unit size: soft 400, hard 800` (those defaults when absent). `ns-triage` records a size estimate in every brief and splits work past the soft limit, or holding more than one shippable behaviour, into child issues with `Blocked by` edges; the parent goes to `needs-define` (labels untouched while `status:in-progress`, where `brief.md` gets `status: blocked`) and nothing closes. `ns-build` writes `blocked` ("unit too large, split it") when its diff passes the hard limit before the criteria are met.

## Reason

Large units made review slow and fix loops long (#116). Splitting at triage is cheaper than discovering size in review. The parent stays open so a human sees the split; under `gates: auto` the children go straight to `ready-for-agent` with the parent's priority. A blocked build is re-triaged by a human. The unit-size line is parsed by the model; no CLI code backs it. The limits are defined once in `skills/ns-contract/SKILL.md`. Triage sizing has eval cases for the split (`broad-feature-split`) and for the paths that don't split (`small-feature-no-split`, and the Size line in `bug-ready-for-agent`); the build stop needs a model run that writes past 800 lines and has none. Amended by D34, D35 and D37.
