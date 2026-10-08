# CLI for agents

Rules for any CLI an agent drives: a project's control CLI, and the `ns` CLI itself. Human-oriented CLIs block agents with interactive prompts, huge upfront docs, and help text with no copy-pasteable examples. These rules make a CLI work headless and compose in pipelines.

Adapted from cursor/plugins `cli-for-agents`, plus the control-CLI requirements in `docs/DESIGN.md`.

## Non-interactive

- Every input is expressible as a flag, an argument, or stdin. Nothing needs arrow keys, menus, or timed prompts.
- A missing required value is an error with a correct invocation, never a prompt. If a CLI also serves humans, the prompt is the fallback only when stdin is a TTY and no flag was given.

```text
Bad:  control-app flash         ->  ? Which board? (use arrow keys)
Good: control-app flash --board nrf52840dk --image build/app.hex
```

## Layered `--help` with examples

- Running the bare command lists the subcommands with one line each. Each subcommand owns its own `--help`. Unused commands stay out of the agent's context.
- Every `--help` ends with an **Examples** block of real invocations. Examples do more for pattern-matching than prose.

```text
usage: control-app serial expect [--port PORT] [--timeout S] PATTERN

Options:
  --port      Serial device (default: from doctor, e.g. /dev/ttyACM0)
  --timeout   Seconds to wait for PATTERN (default: 5)

Examples:
  control-app serial expect "boot ok"
  control-app serial expect --timeout 30 "app v1\.4\.2"
```

## Structured output

- Success prints JSON on stdout: IDs, paths, URLs, durations, versions. One object per invocation, or one per line for streams.
- Diagnostics and progress go to stderr, so stdout stays parseable.
- Artifacts (logs, captures, transcripts) are written to files, and the JSON names their paths.

```json
{"ok": true, "command": "flash", "image": "build/app.hex", "sha256": "9f2c...", "duration_s": 4.1, "log": ".ns/verify/r1/flash.log"}
```

## Actionable errors, fast

- Fail immediately on a missing or bad flag. Print what was wrong and the correct invocation. Exit non-zero.
- When the fix is another command, name it.
- Exit codes: `0` success, `1` the operation or check failed, `2` usage error. Document any others in `--help`.

```text
error: no probe found
  check:  control-app doctor
  then:   control-app flash --image build/app.hex
```

## Idempotent

- Agents retry. Running the same successful command twice is safe: a no-op that reports `"status": "already-running"` or `"already-done"`, never a duplicate side effect.
- `launch` on a live instance reports it. `cleanup` with nothing to clean succeeds.

## `--dry-run` on anything destructive

- Anything that kills, deletes, flashes, erases, or writes outside the evidence dir takes `--dry-run`, which prints the plan as JSON and changes nothing.
- Offer `--yes` to skip a human confirmation while keeping the safe default.
- A dry-run must actually be dry. If it opens a port or touches the network, say so in `--help`.

## `doctor` and `cleanup`

- `doctor` is read-only and answers "is this instance worth driving?". It reports each check with `ok`, `detail`, and a `fix` command when it fails. Exit `1` if any check fails.
- `cleanup` tears down only what this CLI started, tracked by pidfile or session name in its state dir, never by process name. It keeps the evidence dir.

## Predictable structure

- One shape everywhere, such as `noun verb`: if `serial send` exists, `serial expect` and `http get` follow it.
- Accept stdin where it fits (`control-app serial send --stdin < script.txt`) and support chaining (`control-app flash --image "$(control-app build | jq -r .image)"`).
- A few composable commands that each do real work beat many thin ones.

## Review checklist

When writing or reviewing a CLI, check each:

- [ ] Every input works as a flag, argument or stdin. No prompt on the agent path.
- [ ] Bare command lists subcommands. Every subcommand has `--help` with Examples.
- [ ] Success output is JSON on stdout. Diagnostics go to stderr.
- [ ] Errors exit non-zero and print a correct invocation or the next command.
- [ ] Repeating a successful command is safe and says so.
- [ ] Destructive commands take `--dry-run`, and it is really dry.
- [ ] `doctor` exists, is read-only, and prints a fix per failed check.
- [ ] `cleanup` exists, kills only what the CLI started, and keeps evidence.
- [ ] Command names follow one consistent shape.
