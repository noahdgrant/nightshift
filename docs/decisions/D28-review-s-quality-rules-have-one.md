---
id: D28
title: "Review's quality rules have one checklist, quality-bar.md"
issue: 150
date: 2026-10-09
---

# D28: Review's quality rules have one checklist, quality-bar.md

## Decision

Review's quality rules have one checklist, `skills/ns-contract/references/quality-bar.md`, one section per review axis. `ns-build` checks its own diff against it before writing `build.md`: every bar item marked, a test for every branch and error path it adds, and a targeted mutation pass using review's mutation procedure, all recorded under `## Self-check`. Reviewers get their axis's section in their brief. `ns-tdd` points to the same test-completeness rule.

## Reason

Every rule review enforced lived only in `ns-review/references/`, so build never had a chance to meet it, and review found about 6 Important findings per unit (#150). The bar lists what must hold; each axis reference still says how to hunt for violations, so the upstream-derived reference files keep their structure. It lives in `ns-contract` because both phases read it. Each bar item carries its severity, and those tags win over the axis references, so a style nit can't block a unit under D27. The model-run eval case `py-self-check-before-handoff` checks the self-check by running the trial's own tests against a boundary mutant and a nothing-to-cancel mutant. `ns-review` and `ns-tdd` get no new case: their change is a pointer to the bar, and their existing cases now install it. Measuring the effect on first-pass yield needs `ns quality` (#147) and is tracked in #159. Spec: skills/ns-build/SKILL.md.
