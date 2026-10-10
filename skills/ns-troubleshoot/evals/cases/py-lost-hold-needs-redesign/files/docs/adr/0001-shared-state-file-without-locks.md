# ADR-0001: One shared state file, no locking

Status: accepted

## Context

Every packing station runs the CLI against one `inv.json` on a network share. The stations mix Linux and Windows clients, and the share doesn't honour advisory file locks across them.

## Decision

The CLI reads the whole state file, changes it in memory, and writes it back atomically (`storage.load`, then `storage.save`). It takes no lock, because a lock the share ignores would only look safe.

## Consequences

Two runs that overlap can each write a state the other never saw. We accept that for now. Revisit this decision, with its trade-offs, before adding any locking or changing the state file format.
