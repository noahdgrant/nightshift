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

- Baseline. Recipes run in order and each one rewrites `$V/repo/.nightshift/`.

- **Missing.** Exit `1`; `errors` says the file does not exist and `ns run` would use the built-in defaults.

  ```bash
  "$V/sandbox" rm -rf "$V/repo/.nightshift"
  "$V/sandbox" cap factory-missing ns factory validate
  ```

- **Valid.** Exit `0`; `name` is `demo` and `phases` lists triage, build, verify, review and ship with skill `ns-<phase>`.

  ```bash
  "$V/sandbox" sh -c 'mkdir -p .nightshift && echo "$1" > .nightshift/nightshift.toml' _ 'name = "demo"'
  "$V/sandbox" cap factory-ok ns factory validate
  ```

- **Bad value.** Exit `1`; `errors` contains `[queue] source = "bitbucket": unknown`. The last line restores the file, or the [run](./run.md) and [watch](./watch.md) recipes fail.

  ```bash
  "$V/sandbox" sh -c 'printf "$1" > .nightshift/nightshift.toml' _ 'name = "demo"\n[queue]\nsource = "bitbucket"\n'
  "$V/sandbox" cap factory-bad-value ns factory validate
  "$V/sandbox" sh -c 'echo "$1" > .nightshift/nightshift.toml' _ 'name = "demo"'
  ```

- **This repo.** Exit `0`.

  ```bash
  "$V/sandbox" --src cap factory-ok-self ns factory validate --factory .nightshift
  ```

## Gotchas

- Without `--factory` it reads the main worktree's `.nightshift/`, not the current worktree's. Pass `--factory` when the change under test is in a unit worktree.
