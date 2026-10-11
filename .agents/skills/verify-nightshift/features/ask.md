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

- Baseline, in `$V/repo`.
- An echo harness config: `printf '[harness.echo]\ncommand = ["sed", "s/^/echo: /"]\n[roles.default]\nharness = "echo"\n' > $V/c-ask.toml`.

- **Dry run.** Run `cap ask-dry "$NS" ask --role review.security --dry-run`. `matched_role` is `default`, `harness` `claude`, `binary` `$V/bin/claude`, `mode` `read-only`.
- **Real run.** Run `echo 'Review this diff' | NS_CONFIG=$V/c-ask.toml "$NS" ask --role review.security`. Stdout is `echo: Review this diff`, exit `0`.
- **No role.** Run `NS_CONFIG=/dev/null "$NS" ask --role build --dry-run`. Exit `3`; stderr says `role "build" is not configured`.
- **No binary.** Write `[harness.gone]\ncommand = ["no-such-harness-bin"]\n[roles.default]\nharness = "gone"` to `$V/c-gone.toml` and run `echo x | NS_CONFIG=$V/c-gone.toml "$NS" ask --role build`. Exit `4`.
- **No write command.** Run `echo x | NS_CONFIG=$V/c-ask.toml "$NS" ask --role build --write`. Exit `5`; stderr says `--write needs a command_write`.

## Gotchas

- The fake `claude` answers only phase prompts (a `Phase: <name>` line). Drive a real `ns ask` through the echo harness above, not the fake.
- A harness name that is neither built in nor under `[harness.*]` is a config error, exit `3`, not exit `4`.
