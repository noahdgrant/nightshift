# ns

The Nightshift CLI. It creates one worktree per unit of work, runs a configured harness headless for a role, lints skills, and installs them into harness skill directories.

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
| 3 | `ns ask`: role not configured, or no config file |
| 4 | `ns ask`: harness binary not on PATH |
| 5 | `ns ask --write`: the harness has no `command_write` |

## Commands

### `ns worktree new <unit-id> [--base <branch>] [--repo <path>]`

Creates `<root>/../<repo-name>.worktrees/<unit-id>` on branch `ns/<unit-id>`, where `<root>` is the main worktree of the repo you run it from. The base defaults to the local branch behind `origin/HEAD`, else the current `HEAD`. It also creates `.ns/<unit-id>/` in the worktree and adds `.ns/` to the common `.git/info/exclude`.

Prints `{"unit","path","branch","artifacts"}`. If the worktree exists, it prints the same JSON. If only the branch exists (after `remove`), it checks the branch out again. Unit ids match `[a-z0-9][a-z0-9-]*`.

### `ns worktree list [--repo <path>]`

Prints a JSON array of worktrees on `ns/*` branches. Each entry has `unit`, `path`, `branch`, `artifacts` and `status`. `status` is `null`, or `{"file","phase","status","updated"}` from the artifact with the newest `updated` in its frontmatter. Ties go to the later phase.

### `ns worktree remove <unit-id> [--dry-run] [--force] [--repo <path>]`

Removes the unit's worktree and keeps its branch. A worktree with uncommitted changes is refused unless `--force` is given. Artifacts under `.ns/` are ignored, so they don't count as changes. Removing a unit that has no worktree prints `"action": "none"` and exits 0.

### `ns ask --role <role> [--prompt-file <f>] [--write] [--cwd <dir>] [--dry-run]`

Runs the harness configured for the role. The prompt is read from `--prompt-file`, or from stdin, and sent to the harness on stdin. The harness's stdout streams through. The default is read-only. `--write` uses the harness's `command_write`, so the harness can edit files and run commands. `--cwd` runs the harness in that directory, such as a unit's worktree. `--dry-run` prints the resolved command, mode, cwd and whether the binary is on PATH, as JSON, without running anything.

The role lookup falls back from the most specific name: `review.security`, then `review`, then `default`.

### `ns lint [path] [--human]`

Checks each `ns-*/SKILL.md` under `path` (default `./skills`):

- the frontmatter parses as YAML
- `name` equals the directory name and starts with `ns-`
- `description` is non-empty and at most 1024 characters
- each `metadata.upstream` entry (a string or a list) matches `owner/repo@<7-40 hex>:path`
- every relative link `](./x)` or `](../x)` in any `.md` file of the skill points to an existing path. Anchors are stripped. Fenced code blocks are skipped.

Prints `{"ok","skills","errors":[{"skill","file","message"}]}` and exits 1 if there are errors. `--human` prints one line per error.

### `ns install [--source <dir>] [--target <dir>]... [--dry-run]`

Symlinks each `<source>/ns-*` directory (default source `./skills`) into each target. The default targets are `~/.agents/skills` and `~/.claude/skills`, and they are created if missing. Each action is one of:

| Action | Meaning |
|---|---|
| `link` | new symlink |
| `unchanged` | already points at the source |
| `relink` | replaced an `ns-*` symlink that pointed at another checkout of the same skill, or at nothing |
| `prune` | removed a dangling `ns-*` symlink into the source, for a skill that was deleted |
| `conflict` | a real directory or an unrelated symlink is in the way. Left alone, and the command exits 1 |

### `ns doctor`

Prints the config path, whether it exists and parses, and each configured role with its resolved harness, model, read-only and write commands, and whether the binary is on PATH. `distinct_harnesses` lists the providers in use, so a skill can tell whether two roles get a cross-provider check. `harnesses` reports which of `claude`, `codex`, `cursor-agent`, `gemini` and `opencode` are on PATH. `problems` lists anything that would make `ns ask` fail.

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
