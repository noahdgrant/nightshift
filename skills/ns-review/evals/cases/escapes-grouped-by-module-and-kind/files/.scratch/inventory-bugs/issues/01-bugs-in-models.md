🤖 *Posted by nightshift, an AI agent.*

# Bugs in src/inventory/models.py

Status: needs-triage
Category: bug
Priority: priority:low

Module: `src/inventory/models.py`. Kind: bug.

- [ ] `src/inventory/models.py:56` (Suggestion, missed by correctness): `Reservation.from_dict` accepts a zero or negative quantity from a state file. Evidence: it calls `int(data["quantity"])` and never checks the sign, while `Warehouse.reserve` refuses a quantity under 1.

Found while reviewing unit 00-load-state.

This escaped an earlier review.

Source: review_escape

## Comments
