# nightshift verification map

This directory is the maintained source for verifying what a user of `ns` sees. Read the index, then use the matching feature file as the recipe. `ns --help` lists every command; a command with no file here is not mapped yet.

## Baseline preconditions

- Launch and Doctor from [../SKILL.md](../SKILL.md) passed, and this shell set `V` to the directory Launch printed.
- `$V/repo` is a git repo on `main` with one commit and a bare `origin` at `$V/remote.git`.
- `$V/config.toml` is the ns config, holding `[roles.default] harness = "claude"` and a `[forge.github]` whose token command echoes `fake-token`.
- `$V/home` is the sandbox's `HOME`, so skill directories and `~/.config` are the sandbox's. Launch installed this checkout's `ns-*` skills into `$V/home/.claude/skills`.
- Never drive a repo, `HOME` or config that this run did not create.

## Driving conventions

- Every command a recipe runs is one line in a fenced `bash` block under `Driving it with ns`, starting `"$V/sandbox" `: [the sandbox](../SKILL.md#the-sandbox) is the only way to drive. No `cd`, no `export`, no chaining outside single quotes, no glob, and a `.ns/` path only as one unit's folder. `scripts/check-verify-recipes.sh` fails CI on a recipe line that breaks these rules. A command shown in prose or under `Gotchas` is not a recipe.
- `"$V/sandbox"` runs in `$V/repo`, for commands that act on a target repo. `"$V/sandbox" --src` runs in this checkout, for commands that act on nightshift's skills (lint, eval, install).
- Start each recipe from the baseline unless its preconditions say otherwise. Recipes in a file run top to bottom and clean up the state they add; a recipe that needs a state another file leaves (a worktree, a `.nightshift/`) says so, and a fresh sandbox is always safe.
- Prefer JSON field names and exit codes over human-readable text. Read results with `jq` through the sandbox: `"$V/sandbox" sh -c 'jq .ok "$OUT/<id>.out"'`.
- A recipe that changes config writes its own file (`$V/c-<feature>.toml`) and passes it with `env NS_CONFIG=...` on that one command, so the baseline config stays as it is.
- Seed fake forge state with the files each fake's header in `cli/tests/fixtures/factory/` lists, under `$V/ghd`. Pass file contents as an argument: `"$V/sandbox" sh -c 'echo "$1" > "$V/ghd/<file>"' _ '<text>'`.

## Proof and skip reporting

- CLI proof is the command, stdout, stderr and exit code: `"$V/sandbox" cap <name> <command...>` saves all four in `$OUT`.
- Every command a recipe asserts on goes through `cap`, named with the sub-feature ID (add a suffix when one ID has several captures). Only setup commands run without it. Env vars: `cap <id> env VAR=x ns ...`. Stdin: `cap <id> sh -c 'echo x | ns ...'`. Both: `cap <id> env NS_CONFIG=... sh -c 'echo x | ns ...'`, with the env prefix before `sh`, never inside the quotes.
- A command that writes gets a second, read-only capture of the state it changed (`ns worktree list`, `git branch`, `ls`, `cat "$V/ghd/labels-<n>"`).
- A command that would start a harness or reach a forge gets the fake's record as proof: `$V/ctrl/calls`, `$V/ghd/calls`, `$V/ghd/glab-calls`.
- Record the feature ID and entry point with every artifact.
- Report an entry point you could not reach with the command tried and the unmet precondition. Never report it as verified through another entry point.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph on the user-visible behaviour. Then exactly four H2 sections, in this order:

1. `Sub-features`: short IDs, one line each.
2. `How to get to it (user POV)`: every user entry point.
3. `Driving it with ns`: starts with `Preconditions:`, then labelled bullets pairing each action's observable result with the exact commands, in a fenced `bash` block of `"$V/sandbox"` lines under the bullet.
4. `Gotchas`: traps that waste or invalidate a run.

## Features

- [Worktrees](./worktree.md): `ns worktree new`, `list`, `setup` and `remove`.
- [Ask](./ask.md): `ns ask` runs a role's harness headless.
- [Lint](./lint.md): `ns lint` checks `ns-*` skills.
- [Install](./install.md): `ns install` links skills into harness skill directories.
- [Doctor](./doctor.md): `ns doctor` reports config, roles, harnesses, skill dirs and forges.
- [Eval](./eval.md): `ns eval --dry-run` plans skill evals.
- [Factory validate](./factory-validate.md): `ns factory validate` checks `.nightshift/nightshift.toml`.
- [Run](./run.md): `ns run` drives one unit through every phase.
- [Watch](./watch.md): `ns watch` works the ready queue.
- [Forge config](./forge-config.md): `[forge.*]` tokens and `[queue] source`, as `ns doctor` and `ns watch` read them.
