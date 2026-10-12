---
id: D42
title: "The triage pass has no count cap"
issue: 232
date: 2026-10-10
amends: [D31]
---

# D42: The triage pass has no count cap

## Decision

Amends D31. The triage pass has no count cap: `[queue] triage_per_night` is removed, and a file that still sets it loads with a warning, not an error. Candidates sort by `priority` label, highest first, then issue number, with no `order` label. Before each unit the pass triages every candidate no earlier pass that night listed, then the backlog in that order while the queue has no ready issue; the rest of the backlog waits for later passes. At the night's first pass every candidate is backlog. `--until`, the budget and the harness breaker bound the pass as before, and each candidate is still tried once a night. `--dry-run` shows `triage_pass` and `triage_next_pass` in place of the cap.

## Reason

The cap of 10 was the wrong limit: triage-only runs are cheap next to units, and on 2026-10-10 32 issues went back to triage, three nights' work at 10 a night (#232). With no cap, time and money bound the pass, which are what the operator cares about. Interleaving keeps the first unit from waiting on a big backlog: the pass stops once anything is ready, and a later pass picks the backlog up when the queue runs dry. New issues go first because they are most often tonight's escapes and follow-ups, which D31 wants built the same night. Sorting on priority alone, not the queue's category, since triage may change the category, and a priority an escape got from its severity is the best signal before triage. A ready issue that is blocked, or that the queue skips for any reason, does not count, so it can't stall triage. Floods are limited where issues are created (the audit's issues per run, #214; one escape per finding), not here. A warning, not an error, for the old key, so a file written for an older `ns` still runs tonight. Spec: docs/factory/watch.md ("Triage pass").
