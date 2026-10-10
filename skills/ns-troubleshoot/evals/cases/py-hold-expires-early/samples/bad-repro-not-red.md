---
unit: 02-holds-expire-early
phase: troubleshoot
status: pass
base: main
updated: 2026-10-09T19:15:54Z
---
Issue: https://github.com/example/repo/issues/2

## Agent Brief
**Category:** bug
**Route:** ns-build
**Summary:** A hold reloaded from the state file expires early.
**Current behavior:** fulfil refuses before the printed expiry.
**Desired behavior:** fulfil works until the printed expiry.
**Acceptance criteria:**
- [ ] the minimised repro, as a regression test at the seam, goes green
**Out of scope:** none.

## Repro
```bash
export PYTHONPATH=src
python3 -m inventory --state inv.json --now 2026-03-02T09:00:30 add-item BOLT-M6 Bolt
python3 -m inventory --state inv.json --now 2026-03-02T09:00:30 receive BOLT-M6 10
python3 -m inventory --state inv.json --now 2026-03-02T09:00:30 reserve BOLT-M6 4 --ttl 5
python3 -m inventory --state inv.json --now 2026-03-02T09:05:10 fulfil R0001
# ok
```
The seam is `Warehouse.save` then `load`.

## Root cause
`Reservation.to_dict` in `models.py` writes `expires_at` cut to the minute, so a reload loses up to 59 s.
