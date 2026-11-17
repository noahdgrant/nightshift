---
name: ns-cli-design
description: How a command-line tool is built and how its commands, flags, output, errors and exit codes behave: noun verb, a thin edge over a tested library, output as a contract. Use when a unit adds or changes a command, a flag, an output format, an exit code or help text of a command-line tool.
---

# CLI design

Reference only. People and harnesses both script a command line, so what it prints and returns is a contract.

## Architecture

- **Thin edge, library core.** A command parses its arguments, calls a library function, prints the result and returns an exit code. Logic and I/O sit behind the function, so a test calls it. `ns-architecture-design` says where the seams go.
- **One declared tree.** The ecosystem's standard parser holds every command, flag and help text as data, and help, completion and usage errors come from it. Hand-parsed arguments have none of these.
- **The edge takes its world as arguments**: stdout, stderr, the environment, the clock and the working directory, so a test captures them.
- **The binary is tested through its contract.** One test per command runs the built binary with fakes for the tools it calls and asserts stdout, stderr and the exit code as literal values.
- **A test walks the tree** and fails on a command with no help, a verb outside the vocabulary, a required flag, or a flag name that means two things.

## Shape: noun verb

- `tool noun verb [object] [flags]`. The noun is a singular term from the repository's glossary, and a noun alone lists its verbs. No bare verb sits at the top but `help` and `version`.
- **One vocabulary of verbs**, each with one meaning in every noun (`new`, `list`, `show`, `remove`, `check`, `start`, `stop`). `remove` is never `delete` somewhere else.
- **Two levels.** A third level means the noun is two nouns.
- **The object is positional. A flag changes how, never what.** A flag that picks a different action is a verb.
- **A flag is never required.** Every flag is optional and has a default. A value the command cannot run without is an argument, because a required flag is an argument with a longer name.
- **One flag, one meaning** in every command. Long names in kebab-case, the unit in the name (`--timeout-seconds`), a boolean named for the positive.

## Output

- **stdout is the answer and stderr is everything else**: progress, warnings and errors.
- **The reader sets the format.** A command a program reads prints JSON: one object for a snapshot, one line per event for a stream. A command a person reads prints text. One that serves both has `--json`.
- **Fields are stable.** One type each (an id is a number or a string everywhere), the glossary's words, added and never renamed or removed. A document lists them.
- **Off a terminal** there is no colour, spinner or prompt, `NO_COLOR` is honoured, and each line is flushed as it happens.
- **A list has a stable order** and prints the ids the next command takes.

## Exit codes

`0` it did what was asked. `1` it ran and the answer is no, such as a failed check. `2` it could not run, such as a usage mistake or a missing tool. The help states them, and an error never ends in `0`.

## Usage

- **Help** at every level: a one-line summary, an example, the output's shape and the exit codes. A mistyped command or flag names the nearest one.
- **An error** says what is wrong, where the rule is written and what to run next.
- **Safe by default.** A command that only reads changes nothing and says so. A destructive one names what it destroys and needs a flag. One that changes shared state has `--dry-run`. Running it twice is safe.
- **No prompt** unless conversation is the command's purpose. Every answer a command needs is an argument, or an optional flag with a default.
- **Settings in one order**: flag, environment (`TOOL_` prefix), project file, user file, default. A setting is named like its flag, and a setting of one machine never goes in the project file.

## Changing a command

A command, flag, field, exit code and file format is a contract. A rename or removal keeps the old spelling working for one release with a warning on stderr only, carries the breaking-change label, and then deletes the old spelling.

## On evidence only

Shell completion, paging, `--limit`, short flags, aliases.
