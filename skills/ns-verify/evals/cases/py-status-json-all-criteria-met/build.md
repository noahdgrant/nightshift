---
unit: 6-status-json
phase: build
status: pass
sha: @SHA@
updated: 2026-10-10T03:12:00Z
---

## Commits
@SHA@ feat(cli): print stock levels as JSON with status --json

## Tests added
tests/test_cli_status_json.py::test_status_json_lists_every_item_in_sku_order: AC1 (CLI through inventory.cli.main)
tests/test_cli_status_json.py::test_status_json_for_one_sku: AC2 (CLI through inventory.cli.main)
tests/test_cli_status_json.py::test_status_json_for_unknown_sku_fails: AC3 (CLI through inventory.cli.main)

## Checks
python3 -m pytest -q: pass, `26 passed in 0.11s`

## Self-check
Bar: 9 met, 0 fixed, 3 not applicable. performance: no hot path touched. security: no input reaches a shell or file path. comments: none added.
Branches: 2 listed, all tested. src/inventory/cli.py `if args.json`: test_status_json_lists_every_item_in_sku_order. `else` (table): tests/test_cli.py::test_reserve_and_status.

| Mutation | Location | Command | Result |
|---|---|---|---|
| `if args.json` to `if not args.json` | `src/inventory/cli.py` | `timeout 60 python3 -m pytest -q tests/test_cli_status_json.py tests/test_cli.py` | killed |

## Deviations from the brief
none

## Open risks
none
