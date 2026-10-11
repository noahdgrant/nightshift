---
id: D38
title: "ns-triage sends an issue with an open design decision to needs-define under either gate"
issue: 210
date: 2026-10-10
---

# D38: ns-triage sends an issue with an open design decision to needs-define under either gate

## Decision

`ns-triage` sends an issue with an open design decision to `needs-define` under either gate. A decision is open when the issue leaves the implementer to choose between listed approaches, invent a mechanism, format, public interface or tool that neither the issue, the repo's docs nor an existing code pattern specifies, or make another choice that changes how users or other phases work. Only a team member's own words settle one: the issue body, a comment, or under `gates: stop` an answer in the triage session. Triage's own earlier text (Triage Notes, briefs, split children) never counts. The brief records each settled decision and who made it.

## Reason

A clear, small issue whose criteria said "pick the mechanism and record it in DESIGN.md" was triaged `ready-for-agent` (#210), so the factory would have made a design call a human should make (#216). Size and clarity don't catch it: the issue is small and its intent is plain. Triage's own recommendation never counts, so an overnight run can't settle the question it was meant to escalate, and text from outside the team stays data. Choices inside a named design (names, structure, module) stay with the builder, so a fully specified issue still goes straight to build. Known limit: an issue triaged while `status:in-progress` writes a `blocked` brief (step 6), so `ns watch` moves it to `ready-for-human`, not `needs-define`; either way a human picks it up. Eval cases `design-choice-needs-define` and `design-chosen-ready-for-agent` cover both sides. Spec: skills/ns-triage/SKILL.md.
