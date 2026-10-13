# Status still counts a reservation after it expires

Status: needs-triage
Category:

Reported by: Dana (warehouse ops)

## Description

Once a reservation has run out, `reserve` lets me hold that stock again, but `status` still shows it as reserved and the available column stays low.

## Steps to reproduce

```
python3 -m inventory --state /tmp/inv.json --now 2026-03-02T09:00:00 add-item BOLT-M6 "M6 hex bolt"
python3 -m inventory --state /tmp/inv.json --now 2026-03-02T09:00:00 receive BOLT-M6 10
python3 -m inventory --state /tmp/inv.json --now 2026-03-02T09:00:00 reserve BOLT-M6 4 --ttl 10
python3 -m inventory --state /tmp/inv.json --now 2026-03-02T09:30:00 status
```

## Expected

At 09:30 the reservation has expired, so `status` shows BOLT-M6 with 0 reserved and 10 available.

## Actual

```
SKU           ON HAND RESERVED AVAILABLE
BOLT-M6            10        4         6
```

## Comments
