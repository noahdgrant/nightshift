---
unit: 06-holds-lost-between-stations
phase: troubleshoot
status: blocked
base: main
updated: 2026-10-10T06:00:00Z
---
needs redesign: route ns-define, because stopping the lost update needs locking or a state file revision, and ADR-0001 rules out locks.
Issue: .scratch/reservations/issues/06-holds-lost-between-stations.md

## Agent Brief
**Category:** bug
**Route:** ns-define
**Summary:** Two stations reserving at once lose one hold and reuse its ID.
**Current behavior:** the second save overwrites the first; both stations print the same reservation ID.
**Desired behavior:** every reserve keeps its hold and gets its own ID, or fails and says so.
**Acceptance criteria:**
- [ ] the minimised repro, as a regression test at the seam, goes green
**Out of scope:** none.

## Repro
```bash
export PYTHONPATH=src
python3 -m inventory --state inv.json --now 2026-03-02T09:00:00 add-item BOLT-M6 Bolt
python3 -m inventory --state inv.json --now 2026-03-02T09:00:00 receive BOLT-M6 10
python3 - <<'PY'
from datetime import datetime
from pathlib import Path
from inventory import storage
p, now = Path("inv.json"), datetime(2026, 3, 2, 9)
a, b = storage.load(p), storage.load(p)
ra = a.reserve("BOLT-M6", 2, now); storage.save(a, p)
rb = b.reserve("BOLT-M6", 3, now); storage.save(b, p)
print(ra.id, rb.id, [(r.id, r.quantity) for r in storage.load(p).reservations()])
PY
# R0001 R0001 [('R0001', 3)]
```
The seam is `storage.load` then `storage.save`, interleaved for two stations.

## Root cause
Each CLI run does `storage.load`, changes the warehouse, then `storage.save`, with no lock and no revision check. Two runs that overlap both load the same state, and the last write wins: it overwrites the other station's hold and reuses its `next_id`. ADR-0001 chose no locking because the share doesn't honour file locks.
