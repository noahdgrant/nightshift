# inventory

Stock and reservation tracking for a single warehouse. Items are received into stock, held by time-limited reservations, and shipped by fulfilling a reservation.

```
python3 -m inventory --state inv.json add-item BOLT-M6 "M6 hex bolt"
python3 -m inventory --state inv.json receive BOLT-M6 100
python3 -m inventory --state inv.json reserve BOLT-M6 20 --ttl 30
python3 -m inventory --state inv.json status
```

Every command accepts `--now <ISO timestamp>` to act at a fixed time.

## Development

No runtime dependencies. Tests need pytest:

```
python3 -m pytest -q
```
