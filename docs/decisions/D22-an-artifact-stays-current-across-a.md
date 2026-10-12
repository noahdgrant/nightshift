---
id: D22
title: "An artifact stays current across a rebase by patch-id"
issue: 69
date: 2026-10-08
---

# D22: An artifact stays current across a rebase by patch-id

## Decision

An artifact stays current across a rebase: its `sha` is current when it is HEAD or when the unit's diff against the base has the same `git patch-id` as HEAD's. `ns run`'s state table, archiving and merge step share the one check.

## Reason

A rebase onto a newer main used to end a reviewed unit as "needs a human merge" (#69). A diff that changed after review sends the unit back through verify, review and ship, bounded by `max_attempts`. Two choices go beyond the issue's literal text: the merge matches the PR head (including the pre-update head after `BEHIND`), and the base defaults to the checked-out branch, never the literal `HEAD`. `git patch-id` ignores whitespace, so a whitespace-only change after review stays current; whitespace-significant files (Makefiles, Python, YAML) can change behavior that way unreviewed.
