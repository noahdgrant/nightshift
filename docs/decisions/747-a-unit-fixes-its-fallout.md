---
id: ADR-747
title: "A unit fixes its fallout, the rest of its class and small old bugs in touched files; ns artifact check enforces it"
issue: 747
date: 2026-10-15
amends: [D32]
---

# ADR-747: A unit fixes its fallout, the rest of its class and small old bugs in touched files

## Decision

`ns-build` sorts each finding outside the brief into three kinds. **Fallout** is caused by the diff (a consumer of a new event, config key or format, docs it makes stale, baseline entries, helper copies, a size-bar breach, a form of a new ban it leaves out): build fixes every one before the gate and records it as a `fallout:` line naming the commit. **An old bug in a touched file**, fixable in about 30 changed lines, is fixed in the unit with its own commit and test. **The rest**, pre-existing and outside the diff's files or larger than those bounds, stays a D32 `file:` line, tagged `unrelated: <reason>` or `too-big: <lines>`. An instance of the brief's own defect class that the brief missed is fixed too, unless that adds more than about 150 changed lines.

`ns artifact check` on a build artifact rejects a `file:` line that names a file the unit's diff touches (from `brief.md`'s `base:`, else `origin/HEAD`, to `HEAD`) unless it carries `unrelated: <reason>` or `too-big: <lines>`. `ns-review` reads `build.md`'s `file:` lines after synthesis and turns one that meets the changed-code rule into an Important changed-code finding; spec review and the quality bar no longer count fallout and touched-file fixes as scope creep. A changed-code Suggestion is fixed in a fix cycle when cheap, or recorded as `fallout:` in `review.md` and filed by ship. `ns-ship` checks the unit's own escape and follow-up issues for the same location before filing, and tags a filed `unrelated:` line `Source: incidental`, which `ns quality --bugs` counts apart.

## Reason

Under D32 every out-of-scope finding was filed, so units merged half done: the event a diff added went unhandled by its consumer, the docs it made stale stayed stale, and each follow-up issue cost a whole unit to close (#747). The bounds keep the diff reviewable: 30 lines for an old bug and 150 for a class instance are small next to the unit soft limit of 400. The CLI check makes the common mistake, filing a bug in a file the diff already edits, fail at build instead of relying on prose; the tags keep a real exception cheap to state. The check reads the diff only from `ns artifact check`, not from `ns run`'s routing, so an artifact read outside its worktree still routes.

The build eval case D32 named, `py-out-of-scope-defect-marked-for-filing`, is gone: its low-stock report lives in the same file as the defect, which now makes the defect the unit's to fix. `py-new-field-updates-its-consumer`, `py-missed-sibling-instance-fixed` and `py-unrelated-bug-filed-unrelated` replace it, with `disguised-fallout-file-line-is-a-finding` for review and `github-risk-on-an-escape-is-not-filed-twice` for ship.
