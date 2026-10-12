---
id: D31
title: "ns watch runs a triage pass before each unit"
issue: 98
date: 2026-10-09
amended_by: [D42]
---

# D31: ns watch runs a triage pass before each unit

## Decision

Before each unit, `ns watch` runs a triage pass: the triage phase alone, once each, on open team-authored issues that carry `[queue] triage_label` (`status:needs-triage`) or no state label, best priority first, up to `[queue] triage_per_night` runs a night (default 10). A triage-only run never sets `in_progress_label`, so `ns-triage` applies the state and priority itself; its prompt says `Triage only` and the skill stops there, with no hand-off. An issue is tried once a night, whatever came of it. Triage-only runs share the night's budget, `--until`, usage-limit sleeps and harness breaker with units, but are not units for `--once` and `max_units`. A run that can't start is recorded and skipped. Under `gates = "stop"` the pass is off. Amended by D42.

## Reason

Review files escapes and follow-ups as `needs-triage` issues (D29), and only ready issues were queued, so tech debt piled up untriaged (#98). Triaging before each pick, not once at the start, lets an escape filed by tonight's first unit be triaged and, when ready, built the same night, sorted in by its priority. Running the pass first spends the night on the debt the operator cares most about clearing; the cap stops a flood of issues from eating the night. #98 asked only for unlabelled issues nightshift filed; the pass takes every unlabelled team issue, since GitHub can't tell which a phase filed and an unlabelled team issue is untriaged work either way, and the cap bounds the cost. Trying each issue once a night stops a triage that leaves an issue untriaged (needs a human, or a failed run) from looping. Leaving `in_progress_label` off keeps the split in `docs/agents/triage-labels.md`: skills set the states, `ns watch` only its run states. Spec: docs/FACTORY.md ("Triage pass").
