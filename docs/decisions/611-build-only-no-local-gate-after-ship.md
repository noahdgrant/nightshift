---
id: ADR-611
title: "Under [gate] local = \"build-only\", no local gate runs after ship; GitHub checks decide"
issue: 611
date: 2026-10-13
amends: [ADR-466]
---

# ADR-611: Under `build-only`, no local gate runs after ship

## Decision

Under `[gate] local = "build-only"`, a ship that moves HEAD (it merged the base in, or resolved a conflict) runs no local gate (#611). The gate after build runs the full command, the gates after verify and review run `<base> --fast` and the `ci-affected` row, and nothing runs after ship or in the merge step. GitHub's checks on the PR head decide in the merge step (ADR-466). `every-step` is unchanged: a ship that moves HEAD still runs the full gate.

## Reason

The merge step waits for CI on the same head the gate after ship would have tested, so the local run repeated that work. Six such runs averaged 5.8 minutes and none was red. A red check still goes back to build, named, from the merge step.
