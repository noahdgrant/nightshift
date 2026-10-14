---
unit: 44-widget-title
phase: verify
status: pass
sha: SHA
updated: 2026-10-10T20:40:00Z
---

| AC | Criterion | Verdict | Evidence |
|---|---|---|---|
| AC1 | `sh scripts/widget.sh title` prints `Widgets` | PASS | below |
| AC2 | The PR body names #46 as the follow-up for the footer title | ship | PR, checked by ns-ship |

## AC1: `sh scripts/widget.sh title` prints `Widgets`

pass.

```
$ sh scripts/widget.sh title
Widgets
```
