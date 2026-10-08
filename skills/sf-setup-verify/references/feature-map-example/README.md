# Notes verification map

This directory is the maintained source for verifying the user-facing behavior of Notes, a Python service with an HTTP API and a `notes` CLI. Read the index before driving the app, then use the matching feature file as the recipe.

## Baseline preconditions

- Launch Notes with `control-notes launch`. It serves `http://127.0.0.1:4173` from a disposable data directory.
- `control-notes launch` sets `NOTES_DATA_DIR=.sf/verify/state/data` so concurrent runs do not share state.
- Seed notes titled `Quarterly plan` and `Grocery list` with `control-notes seed`.
- Put `control-notes` and the `notes` CLI on `PATH`.
- Run `control-notes doctor` and require the expected URL, data directory, and build revision.
- Never drive an instance that was not started by this verification run.

## Driving conventions

- Start every recipe from the baseline state unless its preconditions say otherwise.
- Prefer route paths, command names, and JSON field names over log text or output position.
- Treat every command as literal. Keep quoted names and flags unchanged.
- Run HTTP actions through `control-notes http <verb> <path>`.
- Run terminal actions through `control-notes exec --name <id> -- <command>`.
- Restore seeded data after a mutation with `control-notes seed --reset`. Do not remove proof artifacts during cleanup.

## Proof and skip reporting

- Capture the user action and the resulting state, not only the final output.
- HTTP proof includes the request, status code, and response body.
- CLI proof includes the command, stdout, stderr, and exit code.
- Mutation proof includes a read-only second view of the stored value.
- Record the feature ID and entry point used with every artifact.
- Report an unreachable path with the attempted command and the unmet precondition.
- Do not report a skipped entry point as verified through a different path.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph describing the user-visible behavior. It then uses exactly four H2 sections in this order.

1. `Sub-features` lists short IDs with one line for each behavior.
2. `How to get to it (user POV)` lists every user entry point.
3. `Driving it with <harness>` starts with `Preconditions:` and uses labeled bullets that pair each user action with an exact command and observable result.
4. `Gotchas` lists traps that can waste or invalidate a verification run.

Keep implementation details out of the map. Name only user paths, stable handles, required state, commands, and observable proof.

## Features

- [Create a note](./create-note.md) covers HTTP and CLI creation, validation, persistence, and cleanup.
- [Search notes](./search.md) covers HTTP and CLI search with matching, empty, and archived states.
