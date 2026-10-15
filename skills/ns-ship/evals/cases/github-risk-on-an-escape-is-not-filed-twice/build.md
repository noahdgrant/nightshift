---
unit: 44-widget-title
phase: build
status: pass
sha: SHA
updated: 2026-10-10T20:30:00Z
---

## Commits
fix(widget): capitalise the title

## Tests added
scripts/test.sh title check: AC1 (CLI)

## Checks
sh scripts/test.sh: pass, `1 passed`

## Deviations from the brief
none

## Open risks
- file: `scripts/admin.sh` deletes the wrong directory when its argument is empty or has spaces: `scripts/admin.sh:3`, `rm -rf $dir/cache` leaves `$dir` unquoted and unchecked, so `sh scripts/admin.sh` runs `rm -rf /cache`, unrelated: the diff only touches `scripts/widget.sh`
