feat: greet.sh greets by name

## Requirements

### Must

- `./greet.sh <name>` prints `Hello, <name>!` and exits 0.
- With no name it exits 2 and prints a usage line to stderr.

### Should

- A greeting is shown in the language of the user's locale as the locale service reports it. The fixture has no locale service and no network, so this cannot be run.
