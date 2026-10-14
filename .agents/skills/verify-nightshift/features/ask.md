# Ask

`ns ask` runs the harness configured for a role headless, with the prompt on stdin and the answer on stdout. Roles fall back from `review.security` to `review` to `default`, and each failure mode has its own exit code.

## Sub-features

- `ask-dry` prints the resolved role, harness and command as JSON.
- `ask-run` pipes the prompt to the harness and prints its answer.
- `ask-no-role` exits `3` when no role or default matches.
- `ask-no-binary` exits `4` when the harness binary is not on PATH.
- `ask-no-write` exits `5` for `--write` on a harness without `command_write`.

## How to get to it (user POV)

- `echo <prompt> | ns ask --role <role>`
- `ns ask --role <role> --prompt-file <file> [--write] [--cwd <dir>] [--dry-run]`

## Driving it with ns

Preconditions:

- Baseline.
- An echo harness config:

  ```bash
  "$V/sandbox" sh -c 'printf "$1" > "$V/c-ask.toml"' _ '[harness.echo]\ncommand = ["sed", "s/^/echo: /"]\n[roles.default]\nharness = "echo"\n'
  ```

- **Dry run.** `matched_role` is `default`, `harness` `claude`, `binary` `$V/bin/claude`, `mode` `read-only`.

  ```bash
  "$V/sandbox" cap ask-dry ns ask --role review.security --dry-run
  ```

- **Real run.** Stdout is `echo: Review this diff`, exit `0`.

  ```bash
  "$V/sandbox" cap ask-run env NS_CONFIG="$V/c-ask.toml" sh -c 'echo "Review this diff" | ns ask --role review.security'
  ```

- **No role.** Exit `3`; stderr says `role "build" is not configured`.

  ```bash
  "$V/sandbox" cap ask-no-role env NS_CONFIG=/dev/null ns ask --role build --dry-run
  ```

- **No binary.** Exit `4`.

  ```bash
  "$V/sandbox" sh -c 'printf "$1" > "$V/c-gone.toml"' _ '[harness.gone]\ncommand = ["no-such-harness-bin"]\n[roles.default]\nharness = "gone"\n'
  "$V/sandbox" cap ask-no-binary env NS_CONFIG="$V/c-gone.toml" sh -c 'echo x | ns ask --role build'
  ```

- **No write command.** Exit `5`; stderr says `--write needs a command_write`.

  ```bash
  "$V/sandbox" cap ask-no-write env NS_CONFIG="$V/c-ask.toml" sh -c 'echo x | ns ask --role build --write'
  ```

## Gotchas

- The fake `claude` answers only phase prompts (a `Phase: <name>` line). Drive a real `ns ask` through the echo harness above, not the fake.
- A harness name that is neither built in nor under `[harness.*]` is a config error, exit `3`, not exit `4`.
