# Docs

Where this repo keeps requirements and design docs, and how they are reviewed. `ns-requirements` reads this file.

## Requirements

- **Directory**: `[docs/requirements/]`. One directory per thing, named for it in lowercase (`<directory>/<name>/`), holding one file per version (`v1.md`, `v1.1.md`) and one `changelog.md`.
- **Template**: `docs/agents/templates/requirements.md` when it exists, else the skill's own. Put the team's house format there to override it.

## Design docs

- **Directory**: `[docs/design/]`.

## IDs

Each requirement ID is `<KEY>-<n>`, where the key names the thing in uppercase (`METER-5`). IDs are never renumbered or reused. Keys in use:

| Thing | Key | Doc |
|---|---|---|
| [thing] | [KEY] | [path to its directory] |

## Review

- **Surface**: [a pull request on this repo | a copy pasted into a shared doc tool, linked from the doc's header]. Review comments come back as edits to the repo file, which stays the source of truth.
