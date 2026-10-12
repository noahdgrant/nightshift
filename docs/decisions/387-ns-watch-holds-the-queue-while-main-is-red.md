---
id: ADR-387
title: "ns watch holds the queue while main is red"
issue: 387
date: 2026-10-11
amended_by: [ADR-388]
---

# ADR-387: ns watch holds the queue while main is red

## Decision

While main is red (D65), `ns watch` holds the queue and the triage pass (#387). It reuses the oldest open team-authored issue titled `fix: main is red at ...`, else files one (`type:fix`, `priority:critical`, the ready label; body: failing checks with redacted log tails, commits since the last green run, the AI-authorship line; log and commit text is fenced, marked untrusted and stripped of `@` mentions), and starts only that. The check runs before each triage-only run and again before the units of a fill start; the run log gets `main_red` and `main_green`. The `Forge` trait gains `create_issue`, `check_log` and `last_green`; each `Check` carries where its log is.

## Notes

Agreed in #377. Matching on the title prefix means a new red sha reuses the open fix rather than filing a second; only team issues count, since anyone may title an issue. Any open team fix issue is reused, even one a person has parked (no ready label, blocked or tried tonight): a second one would pile up beside it each time main goes red at a new sha. The cost is a night that idles when the only fix issue is parked; the night warns that the queue will not start it, and a person readies or closes it. The title is `... at <short sha>: <first failing check>`, with ` (+N more)` when other checks fail. A just-filed issue starts from what the filing returned, as GitHub may not list it yet; a night keeps using the one it filed while it is open, and when the forge can't say whether it is, it counts as open. A CI read that fails mid-night holds nothing, as at start, so a forge outage doesn't idle the night. A unit paused on a usage limit before main turned red resumes with the others that carry on: it already started, and holding it would strand its worktree and in-progress label. A night remembers the heads it filed a fix issue for; a closed one with main still red there is not refiled each fill, since only `--max-units` would bound that, so the night warns once and holds. The forge's area label is left out: nothing names which label that is. Spec: docs/FACTORY.md ("Main's CI").
