# Reports should print ISO dates

Status: needs-triage
Category:
Priority: priority:medium

Reported by: Dana (warehouse ops)

## Description

Every report under `src/inventory/reports/` prints its dates as `DD/MM/YYYY`, each with its own `strftime`. Our partner warehouse reads `02/03/2026` as February the 3rd. Please switch every report to ISO 8601 (`2026-03-02`) and update each report's test to match.

It's the same change in all 30 reports, but there are a lot of lines in them.

## Comments
