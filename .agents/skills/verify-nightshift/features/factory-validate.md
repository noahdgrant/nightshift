# Factory validate

`ns factory validate` parses a repo's `.nightshift/nightshift.toml` and `runners/*.toml`, rejects unknown keys and values, checks `agents/<role>/agent.md`, and prints each phase as it would run.

## Sub-features

- `factory-missing` reports a missing definition and exits `1`.
- `factory-ok` prints every phase with its skill, harness, model, timeout and attempts.
- `factory-bad-value` reports an unknown value, such as `[queue] source`.

## How to get to it (user POV)

- `ns factory validate [--factory <dir>]`

## Driving it with ns

Preconditions:

- Baseline, in `$V/repo`. Recipes run in order and each one rewrites `.nightshift/`.

- **Missing.** Run `rm -rf "$V/repo/.nightshift"`, then `cap factory-missing "$NS" factory validate`. Exit `1`; `errors` says the file does not exist and `ns run` would use the built-in defaults.
- **Valid.** Run `mkdir -p "$V/repo/.nightshift" && printf 'name = "demo"\n' > "$V/repo/.nightshift/nightshift.toml"`, then `cap factory-ok "$NS" factory validate`. Exit `0`; `name` is `demo` and `phases` lists triage, build, verify, review and ship with skill `ns-<phase>`.
- **Bad value.** Write `name = "demo"\n[queue]\nsource = "bitbucket"\n` and run `cap factory-bad-value "$NS" factory validate`. Exit `1`; `errors` contains `[queue] source = "bitbucket": unknown`. Then restore the file with `printf 'name = "demo"\n' > "$V/repo/.nightshift/nightshift.toml"`, or the [run](./run.md) and [watch](./watch.md) recipes fail.
- **This repo.** Run `cap factory-ok-self "$NS" factory validate --factory "$NS_SRC/.nightshift"`. Exit `0`.

## Gotchas

- Without `--factory` it reads the main worktree's `.nightshift/`, not the current worktree's. Pass `--factory` when the change under test is in a unit worktree.
