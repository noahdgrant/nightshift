---
id: ADR-639
title: "An unreadable review artifact no longer keeps a merged unit's worktree"
issue: 639
date: 2026-10-14
amends: [D45]
---

# ADR-639: An unreadable review artifact no longer keeps a merged unit's worktree

## Decision

Cleanup no longer keeps a worktree for "a review artifact no record holds" (D45). When a merged unit's review artifact can't be read (no frontmatter, unreadable file, cycle files with no review artifact), its quality record is saved from the attempts that parsed, the `quality_record` event lists each one under `unparsed` with its unit, path and reason, `ns` warns on stderr that the record is partial, and the worktree is removed. A record that can't be built or saved still keeps the worktree. This holds for `ns clean`, `ns watch`'s sweep and `ns run`'s cleanup after its merge step.

## Reason

One stuck-run note in `history/review-1.md` with no frontmatter kept a merged unit's worktree, 3 GB of build output, on every `ns clean`, with no way out short of editing `.ns/` by hand (#639). Nothing re-reads the artifact later, so keeping the worktree never got the record saved; it only kept the disk. `ns run`'s `record_unit` already saved the record and reported gaps under `unparsed`; cleanup now treats a gap the same way. The run log keeps the path and reason of what the record left out. Rejected: recording the unreadable artifact as a record of its own, since a record is one review attempt and the file holds none that can be read.
