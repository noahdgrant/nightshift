# Create a note

Create note lets a user save a titled note through the HTTP API or the CLI, get a clear error for an invalid note, and read the saved note back from a second user-facing view.

## Sub-features

- `create-http` persists a title and body through `POST /notes`.
- `create-invalid` rejects an empty title with a 422 and an error naming the field.
- `create-cli` creates the same note shape from the terminal.

## How to get to it (user POV)

- Send `POST /notes` with a JSON body.
- Run `notes create --title <title> --body <body>` in a terminal.

## Driving it with control-notes

Preconditions:

- Notes is healthy at `http://127.0.0.1:4173`.
- No note is titled `Release checklist`.
- `control-notes doctor` reports the expected URL and disposable data directory.

- **Create over HTTP.** Post a note. Run `control-notes http post /notes --json '{"title": "Release checklist", "body": "Tag and publish"}' --save create-http`. Status `201` and the body contains an `id` and `"title": "Release checklist"`.
- **Confirm persistence.** Read it back by ID. Run `control-notes http get /notes/<id> --save create-http-readback`. Status `200` and both saved values match.
- **Reject invalid.** Post an empty title. Run `control-notes http post /notes --json '{"title": "", "body": "x"}' --save create-invalid`. Status `422` and the error names `title`.
- **CLI entry.** Create a second note. Run `control-notes exec --name create-cli -- notes create --title "CLI note" --body "Created from terminal" --format json`. Exit code `0` and stdout contain the new note ID and title.
- **Proof.** List all notes. Run `control-notes http get /notes --save create-list`. The saved response contains `Release checklist` and `CLI note`.

## Gotchas

- Titles are trimmed on save. Assert the stored title, not the request value.
- A `201` alone is insufficient proof. Read the note back by ID.
- The CLI defaults to human-readable output. Use `--format json` for stable assertions.
- Run `control-notes seed --reset` after this recipe, but retain the proof artifacts.
