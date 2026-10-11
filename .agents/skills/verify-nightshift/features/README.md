# nightshift verification map

This directory is the maintained source for verifying what a user of `ns` sees. Read the index, then use the matching feature file as the recipe. `ns --help` lists every command; a command with no file here is not mapped yet.

## Baseline preconditions

- Launch and Doctor from [../SKILL.md](../SKILL.md) passed in this shell, so `NS`, `V`, `OUT` and `cap` are set and the fakes in `$V/bin` come first on PATH.
- `$V/repo` is a git repo on `main` with one commit and a bare `origin` at `$V/remote.git`.
- `$NS_CONFIG` is `$V/config.toml`, holding `[roles.default] harness = "claude"` and a `[forge.github]` whose token command echoes `fake-token`.
- `HOME` is `$V/home`, so skill directories and `~/.config` are the sandbox's.
- Never drive a repo, `HOME` or config that this run did not create.

## Driving conventions

- Start each recipe from the baseline unless its preconditions say otherwise.
- Prefer JSON field names and exit codes over human-readable text. Pipe JSON through `jq` for assertions.
- Run commands that act on a target repo from `$V/repo`, and commands that act on nightshift's skills from `$NS_SRC`.
- A recipe that changes config writes its own file (`$V/c-<feature>.toml`) and passes it with `NS_CONFIG=...` on that one command, so the baseline config stays as it is.
- Seed fake forge state with the files each fake's header in `cli/tests/fixtures/factory/` lists.

## Proof and skip reporting

- CLI proof is the command, stdout, stderr and exit code: `cap <name> <command...>` saves all four in `$OUT`.
- A command that writes gets a second, read-only view of the state it changed (`ns worktree list`, `git branch`, `ls`, `$V/ghd/labels-<n>`).
- A command that would start a harness or reach a forge gets the fake's record as proof: `$FAKE_CTRL/calls`, `$V/ghd/calls`, `$V/ghd/glab-calls`.
- Record the feature ID and entry point with every artifact.
- Report an entry point you could not reach with the command tried and the unmet precondition. Never report it as verified through another entry point.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph on the user-visible behaviour. Then exactly four H2 sections, in this order:

1. `Sub-features`: short IDs, one line each.
2. `How to get to it (user POV)`: every user entry point.
3. `Driving it with ns`: starts with `Preconditions:`, then labelled bullets pairing each action with an exact command and an observable result.
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
