# Coding standards

Rules for the `ns` CLI in `cli/` that no lint checks yet.

## One path per dependency

Each outside program the CLI runs has one way to call it for each job. A new call goes through that path, never a fresh `Command`. When none fits, change the path rather than add a second one.

| Dependency | Job | Path |
|---|---|---|
| git | A command on the local repo | `git::run` (trimmed stdout), `git::ok` (success only), `git::command` (the `Proc`) |
| git | A command that talks to `origin`: `fetch`, `push`, `pull`, `clone`, `ls-remote`, `remote update` | `git::run_net` (trimmed stdout), `git::net` (the `Proc`) |

`git::run` refuses the network subcommands with an error naming `git::run_net`. The network path never prompts for credentials, and a stalled transfer or a dead ssh connection ends it (#658).
