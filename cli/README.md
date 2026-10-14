# ns

The Nightshift CLI. It creates one worktree per unit of work, runs a configured harness headless for a role, drives units through the phases unattended (`ns run`, `ns watch`), lints skills, installs them into harness skill directories, and measures them with evals.

## Install

```sh
cargo install --path cli
```

## Conventions

- Machine output is JSON on stdout. Errors go to stderr, with a correct invocation under them.
- Nothing prompts. Every input is a flag, a positional argument, or stdin.
- Commands are safe to re-run. Destructive commands take `--dry-run`.
- `ns <command> --help` lists options and examples for that command.

| Exit | Meaning |
|---|---|
| 0 | ok |
| 1 | failure (git error, lint errors, install conflicts, harness exited non-zero) |
| 2 | usage error (bad unit id, missing path, unreadable prompt file) |
| 3 | `ns ask`: role not configured, or no config file. `ns eval`: eval harness not configured, or the config doesn't parse |
| 4 | `ns ask`, `ns eval`: harness binary not on PATH |
| 5 | `ns ask --write`: the harness has no `command_write` |

`ns run` has its own codes: 0 done, merged or split, 1 stuck, 2 usage or config error, 3 budget, 4 paused on a usage limit, 5 another `ns run` holds the unit's run lock.

## Commands

One page per command, in [docs/cli/](../docs/cli/):

- [`ns worktree`](../docs/cli/worktree.md)
- [`ns ask`](../docs/cli/ask.md)
- [`ns lint`](../docs/cli/lint.md)
- [`ns artifact`](../docs/cli/artifact.md)
- [`ns install`](../docs/cli/install.md)
- [`ns eval`](../docs/cli/eval.md)
- [`ns factory`](../docs/cli/factory.md)
- [`ns score`](../docs/cli/score.md)
- [`ns run`](../docs/cli/run.md)
- [`ns watch`](../docs/cli/watch.md)
- [`ns clean`](../docs/cli/clean.md)
- [`ns log`](../docs/cli/log.md)
- [`ns status`](../docs/cli/status.md)
- [`ns quality`](../docs/cli/quality.md)
- [`ns transcripts`](../docs/cli/transcripts.md)
- [`ns upstream`](../docs/cli/upstream.md)
- [`ns doctor`](../docs/cli/doctor.md)

## Config

Path: `$NS_CONFIG`, else `$XDG_CONFIG_HOME/nightshift/config.toml`, else `~/.config/nightshift/config.toml`.

```toml
# Map roles to a harness and an optional model.
[roles.default]
harness = "claude"

[roles.review]
harness = "claude"
model = "opus"

[roles."review.security"]
harness = "codex"
model = "gpt-5-codex"

# Harnesses: argv for read-only runs, and optionally for --write runs.
[harness.codex]
command = ["codex", "exec", "--sandbox", "read-only", "-m", "{model}"]
command_write = ["codex", "exec", "--sandbox", "workspace-write", "-m", "{model}"]

[harness.gemini]
command = ["gemini", "-m", "{model}"]
```

- `{model}` is replaced with the role's `model`. If the role has no model, an argument that is exactly `{model}` is dropped, along with the flag before it. So `--model {model}` disappears and the harness uses its own default.
- `claude` and `codex` are built in. Define `[harness.<name>]` to override a built-in or to add another harness:

  | Harness | `command` (read-only) | `command_write` |
  |---|---|---|
  | claude | `claude -p --permission-mode plan --model {model}` | `claude -p --permission-mode acceptEdits --model {model}` |
  | codex | `codex exec --sandbox read-only -m {model}` | `codex exec --sandbox workspace-write -m {model}` |

- `acceptEdits` lets Claude edit files without asking, but shell commands still need approval, and a headless run cannot give it. A verify or build worker that has to run tests needs more. Either add `--allowedTools` for the commands it needs, or use `--dangerously-skip-permissions`. That flag lets the agent run any command as your user, so use it only in a disposable worktree or a sandbox.
- Unknown keys are rejected, so a typo fails loudly. Run `ns doctor` after editing.

### `[eval]`

Every key is optional. The defaults are shown:

```toml
[eval]
harness = "claude"            # a table under [eval.harnesses], or the built-in claude
# model = "haiku"             # unset: `--model {model}` is dropped
trials = 2                    # starting trials per arm
max_trials = 5                # adaptive ceiling
budget_usd = 5.0              # per invocation
max_runs = 40
transcripts = "~/.local/share/nightshift/evals"
billing = "subscription"      # or "api"; subscription strips API-key variables and --bare from claude

# Built in; define it to override.
[eval.harnesses.claude]
command = ["claude", "-p", "--model", "{model}", "--output-format", "stream-json", "--verbose", "--permission-mode", "bypassPermissions"]
carry = [".claude/.credentials.json", ".claude.json"]
output = "claude-stream-json"   # or "codex-jsonl" or "none"; the default for a new harness is "none"

[eval.harnesses.codex]
command = ["codex", "exec", "--json", "-m", "{model}", "--sandbox", "danger-full-access", "--skip-git-repo-check"]
carry = [".codex/auth.json"]
output = "codex-jsonl"

[eval.capability.zephyr]      # configured when the table exists
base = "~/zephyrproject/zephyr"
sdk = "~/zephyr-sdk-0.17.0"
path_prepend = ["~/zephyrproject/.venv/bin"]
```

### `[forge]`

Where `ns run` and `ns watch` get forge tokens, so a night doesn't need a manual `export GH_TOKEN=...`:

```toml
[forge.github]
token_command = "gh auth token --user <account>"   # a shell command; its trimmed stdout is the token

[forge.gitlab]
token_env = "MY_GITLAB_TOKEN"                      # or: read this variable
host = "gitlab.example.com"                        # optional; exported as GITLAB_HOST (GH_HOST for github)
```

- Set exactly one of `token_command` and `token_env` per forge.
- At start, `ns run` and `ns watch` resolve each token once and export it as `GH_TOKEN` or `GITLAB_TOKEN`. Every child (`gh`, `git`, each phase's harness) inherits it.
- A variable that is already set and non-empty wins, and no command runs. `ns run`, `ns watch` and `ns doctor` warn when it overrides a configured forge: `GH_TOKEN from the environment overrides the config's choice`. `host` likewise doesn't override a set `GH_HOST` or `GITLAB_HOST`.

An operator with more than one account picks the token per owner or per repo, in the same file:

```toml
[forge.github]                                  # the default
token_command = "gh auth token --user personal-account"

[forge.github.owners."example-org"]             # every repo under an owner
token_command = "gh auth token --user work-account"

[forge.github.repos."example-org/backend"]      # one repo
token_env = "BACKEND_TOKEN"
```

- The owner and repo come from `origin`'s URL: `https://github.com/example-org/backend.git`, `git@github.com:example-org/backend.git`, or an SSH host alias such as `git@github-work:example-org/backend.git`. A local path or `file://` URL names no repo, so the default table applies.
- The most specific table wins: the repo's, then its owner's, then `[forge.github]`. Keys match without regard to case.
- Each override takes `token_command` or `token_env`, exactly one, and no other key. An override needs the default table too, since that's where `host` goes.
- GitLab has the same tables, `[forge.gitlab.owners."<group>"]` and `[forge.gitlab.repos."<group>/<project>"]`. A nested group matches on its full path: `owners."team/sub"` covers `team/sub/project` but not `team/project`. The overrides belong to the one `[forge.gitlab]` table, so with `host` set they are that host's: an `origin` on another host (an SSH alias counts as another host) gets the default table, and `ns run`, `ns watch` and `ns doctor` warn when that drops a table that would have matched (`origin host gitlab-work is not gitlab.example.com: ignoring ...`). The host is `GH_HOST` or `GITLAB_HOST` when set, else `host`. If the origin is an SSH alias for that host, put the token in the default table or drop `host`.
- Overrides apply to the forge `origin` is on (`[queue] source`, else its host, as for `ns watch`). The other forge gets its default table.
- After exporting the token, `ns run` and `ns watch` check that it can read the repo (`gh api repos/<owner>/<repo>`, or `glab api projects/<path>`), but only for a forge with a `[forge.*]` table, and not on `--dry-run`. A forge with no table behaves as before. When the check fails they exit 2 before any phase, naming the account the token belongs to (else where the token came from) and the table, and say to check `ns doctor` if the account is wrong. The check and the account lookup each give up after 15 seconds.
- `NS_FORGE_CHECKED` is set by `ns run` and `ns watch` to `<forge>:<owner>/<repo>` once they have checked the repo, so a unit `ns watch` starts doesn't check again. `NS_FORGE_EXPORTED` is set to the same value whenever they settle the token, checked or not (a dry run, a local-path origin), so a unit doesn't warn that `GH_TOKEN` overrides the config. Only a value equal to the current forge and repo counts; leave both unset, and don't export them in a shell.
- Keep these tables in the user config, never in a committed file, so account names stay out of repos.
- `NS_CONFIG` still selects a whole other config file. That works as an escape hatch, but the overrides are the recommended way: they apply in every shell and in the systemd unit without setting a variable.
- A failing command, empty output or an unset `token_env` stops with exit 2. The error names the forge and the command, never the token or the command's output.
- The token is never written to `runs.jsonl`, transcripts, errors or `ns doctor` output.

### `[runners]`

```toml
[runners]
lock_dir = "~/.local/state/nightshift/locks"   # unset: <git-common-dir>/ns/locks
```

Where `ns run` keeps the locks a factory's runners name. Point every repo that shares a bench or a workspace at the same directory, so their phases wait for each other. See `docs/FACTORY.md`, "Runners".

Harness tables live at `[eval.harnesses.<name>]`, not `[eval.harness.<name>]` as `docs/EVALS.md` shows. TOML can't hold `eval.harness` as both the string `"claude"` and a table, so the spec's example doesn't parse.

A capability holds arbitrary string keys plus an optional `path_prepend` list. Fixture `[env]` values expand `{capability.<name>.<key>}` and `~`. The `path_prepend` entries of every capability a case or its fixture requires go on the front of `PATH` for the whole trial.

The judge uses the normal role config: map `eval.judge` (or `eval`, or `default`) under `[roles]`.
