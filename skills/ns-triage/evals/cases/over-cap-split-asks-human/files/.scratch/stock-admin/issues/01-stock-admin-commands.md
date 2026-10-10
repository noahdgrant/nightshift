# Stock admin commands

Status: needs-triage
Category:
Priority: priority:medium

Reported by: Dana (warehouse ops)

## Description

Running the warehouse day to day needs a set of admin commands the tool doesn't have yet. Each one is small and useful on its own:

1. `inventory rename-item <sku> <name>` changes an item's display name.
2. `inventory remove-item <sku>` deletes an item, refused while it has stock or open reservations.
3. `inventory adjust <sku> <delta> --reason <text>` corrects on-hand stock after a stock count, by a positive or negative amount.
4. `inventory history <sku>` lists every receive, reserve, cancel, fulfil and adjust for an item, oldest first, with timestamps.
5. `inventory status --json` prints the status table as JSON.
6. `inventory extend <reservation-id> <minutes>` pushes a reservation's expiry back.
7. `inventory export <path>` writes all items and stock levels to a CSV.
8. `inventory import <path>` loads items and stock levels from that CSV into an empty state file.

None of these depends on another, except that 8 reads what 7 writes.

## Comments
