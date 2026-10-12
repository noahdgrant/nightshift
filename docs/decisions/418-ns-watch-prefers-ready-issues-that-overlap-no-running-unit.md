---
id: ADR-418
title: "ns watch prefers a ready issue whose touches overlap no running unit"
issue: 418
date: 2026-10-12
---

# ADR-418: ns watch prefers a ready issue whose touches overlap no running unit

## Decision

When a slot frees up, `ns watch` takes the highest-priority ready issue whose predicted `touches` overlap no running unit's, and the highest-priority one when every ready issue overlaps (#418, design agreed in #365). Overlap is a preference, never a block, and `critical` gets no special case. Two units overlap when a glob from each matches one file: a file the repo tracks (`git ls-files`), or a path a glob names outright, so two units adding the same new file overlap too. `[queue] overlap_ignore`, empty by default, lists globs of files that never count. A brief with no `touches` never overlaps. A ready issue's prediction comes from its newest Agent Brief on the forge; a running unit's from `brief.md` in its worktree, which triage may have refined. `--dry-run` names, per ready issue, the units live runs hold that it overlaps.

## Reason

Two units that edit the same files tend to conflict at merge, and the second then reruns or lands as a stuck rebase. Running something else beside a unit avoids that at no cost when the queue has other work. Blocking would idle a slot whenever the queue is one area's work, which costs more than the occasional conflict. Matching against tracked files keeps the rule simple and exact for the common case; glob-against-glob comparison would also catch two units that both create files under a new directory, which is rare and not worth the code.
