# Docs

Where this repo keeps requirements and design docs, and how they are reviewed. `ns-requirements` reads this file.

## Requirements

- **Directory**: `docs/requirements/`. One directory per thing, named for it in lowercase (`docs/requirements/<name>/`), holding one file per version (`v1.md`, `v1.1.md`) and one `changelog.md`.
- **Template**: the skill's own. No override in `docs/agents/templates/requirements.md`.

## Design docs

- **Directory**: `docs/design/`.

## IDs

Each requirement ID is `<KEY>-<n>`, where the key names the thing in uppercase. Keys in use:

| Thing | Key | Doc |
|---|---|---|
| none yet | | |

## Review

- **Surface**: a pull request on this repo. Reviewers comment on the PR; the repo file is the source of truth.
- **Approval**: a human sets Status to Approved, by merging a PR that changes it. Agents leave Status at Draft or In review.
