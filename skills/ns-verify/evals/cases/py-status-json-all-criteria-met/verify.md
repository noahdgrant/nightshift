# Verify

How to prove a change works on the real surface, beyond the test suite. `ns-verify` reads this file.

## Set up

The product is the `inventory` CLI, and this file plus the control CLI below is the verification lever: drive the CLI directly, the way a user would. Verification is set up, so there is no verify skill to generate and no stack-only fallback to take.

- **Control CLI**: `PYTHONPATH=src python3 -m inventory`. Doctor: `PYTHONPATH=src python3 -m inventory --help` exits 0.
- **State**: always pass `--state` with a file in a fresh temp dir, never the default `inventory.json` in the repo. Pass `--now` with a fixed ISO 8601 timestamp so expiry is deterministic.

```bash
S=$(mktemp -d)/inv.json
inv() { PYTHONPATH=src python3 -m inventory --state "$S" --now 2026-03-02T09:00:00 "$@"; }
inv add-item BOLT-M6 "M6 hex bolt"
inv receive BOLT-M6 10
inv status
```

Evidence is the exact commands, their stdout, stderr and exit codes, and the state file when a criterion is about what was saved. Copy artifacts under `.ns/<unit-id>/evidence/`.
