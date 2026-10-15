---
unit: 44-widget-title
phase: review
status: pass
sha: SHA
cycles: 0
updated: 2026-10-10T20:50:00Z
---

## Summary

Open: 0 Critical, 0 Important, 0 Suggestion. Fixed: 0. Dismissed: 0. Escapes filed: 1.
Change size: 7 lines in 2 files.

## Important
### I1. admin.sh removes the wrong directory on an empty or spaced argument
- Location: `scripts/admin.sh:3`
- Axis: correctness
- Scope: pre-existing
- Cycle: 0
- Attempts: 0
- Raised by: correctness (claude).
- Missed by: correctness
- Finding: `rm -rf $dir/cache` leaves `$dir` unquoted and unchecked. With no argument it removes `/cache`; with a spaced path it removes the wrong paths.
- Evidence: `scripts/admin.sh:3` reads `rm -rf $dir/cache`, and `dir=$1` has no check above it.
- Status: deferred: #50
