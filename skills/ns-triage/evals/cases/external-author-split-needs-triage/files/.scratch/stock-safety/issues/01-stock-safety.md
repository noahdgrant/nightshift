# Stock safety

Status: needs-triage
Category:
Priority: priority:medium

Reported by: Sam (customer)

## Description

Two things bit us last month. Either fix helps on its own, and neither needs the other.

1. **Keep a backup of the state file.** A half-written disk lost us a day's receipts. Every save should first copy the existing state file to `<state>.bak`. When the state file can't be read, the error should name the backup so we can restore it by hand.
2. **Cap how long a reservation can hold stock.** Someone reserved with `--ttl 100000` and the stock sat held for two months. `Warehouse.reserve` should refuse a TTL over 7 days with an error that states the limit. The CLI already prints those errors, so it needs no change.

Our ops lead says this needs #99 first.

## Comments
