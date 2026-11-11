feat: report.sh uploads the report to the shared bucket

## Requirements

### Must

- `./report.sh` writes `report.txt` containing `ok`.
- `./report.sh` uploads `report.txt` to the `team-reports` bucket on the company's object store, which needs the `STORE_TOKEN` credential.
