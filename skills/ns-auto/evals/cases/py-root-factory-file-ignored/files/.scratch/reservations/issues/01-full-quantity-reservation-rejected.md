# Reserving the full available quantity is refused

Status: needs-triage
Category:

Reported by: Dana (warehouse ops)

## Description

When I try to reserve all of the stock we have for an item, the reservation is refused even though the error message itself says the stock is there.

## Steps to reproduce

```
python3 -m inventory --state /tmp/inv.json --now 2026-03-02T09:00:00 add-item BOLT-M6 "M6 hex bolt"
python3 -m inventory --state /tmp/inv.json --now 2026-03-02T09:00:00 receive BOLT-M6 5
python3 -m inventory --state /tmp/inv.json --now 2026-03-02T09:00:00 reserve BOLT-M6 5
```

## Expected

A reservation for 5 bolts is created, and `status` shows 0 available.

## Actual

```
inventory: error: cannot reserve 5 of 'BOLT-M6': only 5 available
```

The command exits 1. Reserving 4 works. Reserving 6 is refused, which is correct.

## Comments
