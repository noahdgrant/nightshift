---
id: D35
title: "A split under status:in-progress writes brief.md with status: split"
issue: 206
date: 2026-10-10
amends: [D26]
amended_by: [ADR-391]
---

# D35: A split under status:in-progress writes brief.md with status: split

## Decision

Amends D26. A split under `status:in-progress` writes `brief.md` with `status: split`, a status only `brief.md` takes, and its first body line names the children. `ns run` reads it as a `split` outcome (exit 0, phase triage, reason from that line) ahead of every other row of the state table and moves it to `history/`, and `ns watch` swaps `in_progress_label` for `[queue] split_label` (default `status:needs-define`) and posts no comment. The parent stays open; a human closes it. A split at the cap still writes `blocked`, with the cap reason as its first line.

## Reason

A blocked brief read as stuck, so `ns watch` moved a parent whose three children all merged that night to `ready-for-human` and commented the brief's first line, which was the issue URL (#206). A split needs nothing from a human, so `blocked` was the wrong status; a distinct status keeps `blocked` meaning a blocker and lets the CLI tell the two apart without reading prose. `status:needs-define` is where D26 already puts a parent split outside a run, so a split parent looks the same whichever path triaged it, and the triage pass skips it as a state label. `ns watch` sets the label rather than triage, an exception to D31's rule that skills set the states: removing `in_progress_label` is `ns watch`'s move, so it sets the next state in the same edit, and a local-markdown tracker holds one state per issue. No comment, because triage's notes already list the children. Archiving the split brief means a parent a human sends back to the queue is triaged again, which D34 makes safe, instead of being silently split again from a stale brief. Closing the parent when its children close was rejected for now: triage links children by a body line or a sub-issue, so `ns watch` would need a second tracker query per merge to find the parent, and D26 keeps the parent open so a human sees the split. Cargo tests pin the outcome, the labels, the absent comment and a custom `split_label`; the eval case `split-in-progress-writes-split-brief` checks the brief's status and first line. Spec: docs/factory/watch.md, skills/ns-contract/SKILL.md.
