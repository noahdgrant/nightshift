# Search notes

Search lets a user find notes by title or body text, distinguish no matches from an unavailable search, and choose whether archived notes count.

## Sub-features

- `search-match` returns title and body matches without changing note data.
- `search-empty` returns an empty list, not an error, for a query with no matches.
- `search-archived` excludes archived notes unless the user asks for them.
- `search-cli` returns the same matching notes from the terminal.

## How to get to it (user POV)

- Send `GET /notes?q=<query>`.
- Run `notes search <query>` in a terminal.

## Driving it with control-notes

Preconditions:

- Notes is healthy at `http://127.0.0.1:4173`.
- The disposable data directory contains `Quarterly plan` with body text `Draft budget`, and an archived note `Old budget`.
- `control-notes doctor` reports the expected URL and data directory.

- **Title match.** Search for `quarterly`. Run `control-notes http get "/notes?q=quarterly" --save search-title`. Status `200` and the list contains `Quarterly plan` and not `Grocery list`.
- **Body match.** Search for `budget`. Run `control-notes http get "/notes?q=budget" --save search-body`. The list contains `Quarterly plan` with a body-match excerpt and not `Old budget`.
- **Archived.** Search again including archived notes. Run `control-notes http get "/notes?q=budget&archived=true" --save search-archived`. The list contains both `Quarterly plan` and `Old budget`.
- **Empty state.** Search for an absent value. Run `control-notes http get "/notes?q=volcano" --save search-empty`. Status `200` and the body is `[]`.
- **CLI match.** Run `control-notes exec --name search-cli -- notes search "quarterly" --format json`. Exit code `0` and stdout contain one object whose title is `Quarterly plan`.
- **No mutation.** Run `control-notes http get /notes --save search-after` and compare with the seeded list. Nothing changed.

## Gotchas

- The index updates after a short delay following a write. Wait for the note to appear in `GET /notes?q=` rather than sleeping a fixed time.
- A `503` means the search index is still building after launch. It is not an empty result. Doctor reports index readiness.
- Archived notes are excluded unless `archived=true` (HTTP) or `--archived` (CLI) is passed.
