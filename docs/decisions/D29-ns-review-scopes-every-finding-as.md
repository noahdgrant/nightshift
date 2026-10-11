---
id: D29
title: "ns-review scopes every finding as changed or pre-existing"
issue: 147
date: 2026-10-09
amends: [D27]
amended_by: [D60]
---

# D29: ns-review scopes every finding as changed or pre-existing

## Decision

Amends D27. `ns-review` scopes every finding when it judges it: `changed` (a line the diff adds, a behaviour it changes or removes, or something it leaves out) or `pre-existing`. A pre-existing finding is an **escape**: whatever its severity, including Critical, it never blocks the unit and never goes to the fix loop. Review files it as soon as it is judged, as a `needs-triage` issue with a category and area label, a priority from severity (Critical or any security finding `critical`, falling back to `high` when the tracker has no `critical` role; Important `high`; Suggestion `low`), a link to the unit and the words "escaped an earlier review", and marks it `deferred: #<n>`. Changed-code Criticals and Importants block at the cycle limit as D27 says. Every finding in `review.md` and `cycle-<n>.md` carries `Axis`, `Scope` and `Cycle` fields, so `ns quality` (#147) can count escapes.

## Reason

D27 still blocked on a Critical in code the unit didn't touch, which held a unit back for a defect it neither caused nor owned, and filed only pre-existing Importants, so a Critical or Suggestion found outside the diff had no tracked home (#148). An escape means an earlier review missed something, so it gets an issue at an urgency matching its severity, and the metric that counts escapes needs a parseable field rather than prose. Filing at judgment time, not at the cycle limit, means a unit that passes on its first review still files what it saw. Reviewers are told to report pre-existing defects they notice, and the Noted bucket no longer dismisses a real defect as "outside the diff". The model-run eval case `pre-existing-defect-is-filed-not-blocking` checks a clean diff beside a seeded pre-existing off-by-one: review passes, the finding is `pre-existing` and `deferred`, the local tracker gets the issue, and the defect stays unfixed. Spec: skills/ns-review/SKILL.md. Amended by D60.
