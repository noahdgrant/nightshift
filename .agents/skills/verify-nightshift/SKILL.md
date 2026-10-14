---
name: verify-nightshift
description: Verify nightshift's `ns` CLI and skills on the real surface by driving the built `ns` against a throwaway git repo, with fake `claude`, `gh` and `glab` on PATH. Use when proving a change to `cli/` or `skills/` works the way a user runs it, or when ns-verify asks for this repo's verify skill.
---

# Verify nightshift

The product is the `ns` CLI and the `ns-*` skills. The control CLI is `ns` itself: there is no separate control script. Every drive runs the `ns` built from the worktree under test, inside a sandbox that [`scripts/sandbox.sh`](scripts/sandbox.sh) creates and enters: a throwaway git repo, a bare `origin`, a throwaway `HOME`, and the fake `claude`, `gh` and `glab` from `cli/tests/fixtures/factory/`.

Never call a real model or the real GitHub or GitLab. Every recipe runs through `"$V/sandbox"`, which cuts off `ssh` and the real `gh`, and Doctor checks that the fakes are the binaries on PATH before anything that would start a harness or reach a forge.

Read [features/README.md](features/README.md) before driving, then use the feature file that matches the change.

## Launch

`ns` is a short-lived CLI, so launch means build once and create the sandbox. Run from the root of the worktree under test:

```bash
cargo build --manifest-path cli/Cargo.toml
V=$(.agents/skills/verify-nightshift/scripts/sandbox.sh --new)
echo "$V"
```

[`scripts/sandbox.sh`](scripts/sandbox.sh) `--new` makes the sandbox `$V` with `mktemp` and prints it: a throwaway git repo `$V/repo` on `main` with a bare `origin` at `$V/remote.git`, a throwaway `HOME` at `$V/home` with this checkout's `ns-*` skills installed for claude, the ns config `$V/config.toml`, the fake `claude`, `gh` and `glab`, an `ssh` that always fails, and the `ns` that build wrote (under `$CARGO_TARGET_DIR` when it is set, else `cli/target`). It stops before creating anything when that `ns` or a fake is missing, naming it. Evidence goes in `$OUT` ([Evidence](#evidence)).

Note the printed `$V`; nothing else carries over to later shells. When you run a recipe, write `$V` outside single quotes as that literal absolute path, so the command starts with the path: `<printed dir>/sandbox ns --version`, not `"$V/sandbox" ns --version` or `V=...; "$V/sandbox" ...`. A hook guard denies a command named by a shell expansion when `git` appears in the same command. The recipes below keep the `"$V/sandbox"` placeholder; inside single quotes, `$V` is set by the sandbox.

## The sandbox

`"$V/sandbox" <command...>` is the only way to run a recipe. It runs the command in `$V/repo` with `$V/bin` first on `PATH`, `HOME`, `NS_CONFIG`, `FAKE_CTRL` (`$V/ctrl`), `FAKE_GH_DIR` (`$V/ghd`) and `TMPDIR` in the sandbox, `GIT_SSH_COMMAND=false`, and your `GH_TOKEN`, `GITLAB_TOKEN`, `ANTHROPIC_API_KEY`, `XDG_CONFIG_HOME`, ssh agent and git location variables unset. Inside, `ns` is the built binary, `V`, `NS_SRC` (this checkout), `NS` and `OUT` are set, and `ssh` or the real `gh` fails. `"$V/sandbox" --src <command...>` runs it in `$NS_SRC` instead, for commands that act on this repo's skills (lint, eval, install). It exits with the command's exit code.

Rules for every recipe, and every command you add:

- One line, starting `"$V/sandbox" `. No `cd`, no `export`, no hand-built environment, and no `;` `&` `|` `<` `>`, backtick or `$(` outside single quotes: chain inside `sh -c '...'`. Close every quote on its own line and end no line in a backslash, so a quote never spans lines. Every fence under `Doctor`, `Drive` and `Driving it with ns` is `bash`. `scripts/check-verify-recipes.sh` fails CI on a recipe line that does not.
- Name files by absolute path under `$V`, the literal path outside single quotes and `$V` inside them. `$NS_SRC`, `$OUT` and `$FAKE_CTRL` exist only inside: put them in single quotes under `sh -c`, as in `"$V/sandbox" sh -c 'ls "$FAKE_CTRL"'`.
- No glob (`*`, `?`), bracket, brace or `..` outside single quotes, no `$'...'` or `$"..."` anywhere outside them, and a `.ns/` path only as one unit's folder, `.ns/<unit-id>`. Hook guards and `//**/secrets/**` deny rules block the rest, and the check fails on them.
- Write file contents as an argument, not inside the quotes: `"$V/sandbox" sh -c 'echo "$1" > "$2"' _ '<text>' "$V/<file>"`.
- The throwaway repo's remote is `origin`, and the only token is `fake-token`.

A command a harness denies ends verify `blocked` ([denied commands](../../../skills/ns-verify/SKILL.md#denied-commands)). `ns worktree` puts unit worktrees beside the repo, in `$V/repo.worktrees/`, so they stay inside the sandbox. Each `--new` makes its own `$V`, so two runs can go side by side.

## Doctor

Run before the first drive and whenever anything looks off:

```bash
"$V/sandbox" sh -c 'for b in claude gh glab ssh ns; do [ "$(command -v "$b")" = "$V/bin/$b" ] || echo "NOT FAKE: $b"; done'
"$V/sandbox" ns --version
"$V/sandbox" --src git rev-parse --short HEAD
"$V/sandbox" sh -c 'ns doctor | jq -c "{ok, problems, forge: .forge.github, claude: .harnesses.claude.path, ns_skills: .skill_dirs.claude.ns_skills}"'
```

Worth driving when no `NOT FAKE` line prints, `ok` is `true`, `problems` is `[]`, `forge` shows `"account":"octocat"` and no `ssh`, `claude` is `$V/bin/claude`, and `ns_skills` is the number of `skills/ns-*` directories. Stop on any `NOT FAKE` line: the next `ns run` or `ns watch` would start a real model or call a real forge. `scripts/check-verify-recipes.sh` runs Launch and Doctor in CI and checks these results.

## Drive

Capture each drive with `cap` ([Evidence](#evidence)):

```bash
"$V/sandbox" cap wt-list ns worktree list
"$V/sandbox" --src cap lint-clean ns lint skills
"$V/sandbox" --src cap eval-plan-one ns eval ns-tdd --dry-run
```

Seed the fake forge before `ns run` or `ns watch`. The fakes read files under `$V/ghd`; the header of each fake in `cli/tests/fixtures/factory/` lists them. An open, ready GitHub issue #7:

```bash
"$V/sandbox" sh -c 'echo "$1" > "$V/ghd/issue-7.json"' _ '{"number":7,"title":"Fix the thing","url":"https://github.com/o/r/issues/7","state":"OPEN"}'
"$V/sandbox" sh -c 'echo "$1" >> "$V/ghd/issues.jsonl"' _ '{"number":7,"title":"Fix the thing","labels":[{"name":"status:ready-for-agent"}],"body":"","authorAssociation":"MEMBER"}'
"$V/sandbox" sh -c 'echo status:ready-for-agent > "$V/ghd/labels-7"'
```

The fake `claude` writes each phase's artifact with status `pass` unless `$V/ctrl/<phase>` queues another action (`fail`, `blocked`, `limit`, ...). Its header lists them. It records each phase in `$V/ctrl/calls` and each prompt in `$V/ctrl/prompt-<n>-<phase>.txt`.

The feature files hold the recipe for each command group. `scripts/check-verify-recipes.sh` runs these Drive blocks and then [features/run.md](features/run.md)'s recipes in CI, and fails unless `run-pass` ends `done`.

## Evidence

Evidence goes in `$OUT`: `.ns/verify/<run-id>/` under `$NS_SRC` by default. To use the directory `ns-verify` names (`.ns/<unit-id>/evidence/`), set `OUT` to its absolute path on the `sandbox.sh --new` line: `V=$(OUT="$PWD/.ns/<unit-id>/evidence" .agents/skills/verify-nightshift/scripts/sandbox.sh --new)`.

`"$V/sandbox" cap <name> <command...>` saves the command, stdout, stderr and exit code as `$OUT/<name>.{cmd,out,err,exit}` and prints the exit code. Every command whose result is asserted goes through `cap`, named with the sub-feature ID from the feature file (suffix it when one ID has several captures); only setup commands run without it. Env: `"$V/sandbox" cap <id> env VAR=x ns ...`. Stdin: `"$V/sandbox" cap <id> sh -c 'echo x | ns ...'`. Both: `"$V/sandbox" cap <id> env NS_CONFIG=... sh -c 'echo x | ns ...'`, with the env prefix before `sh`, never inside the quotes. To read a result, run `"$V/sandbox" sh -c 'jq .ok "$OUT/<id>.out"'`. Proof standards:

- Drive the user path: the `ns` command a user types, against a git repo. Calling a Rust function or a test helper is not proof.
- Capture the action and the resulting state. After a command that writes, capture a second, read-only view: `ns worktree list`, `git branch -a`, `ls` of the artifact folder, `cat "$V/ghd/calls"`, `cat "$V/ghd/labels-<n>"`.
- Verify side effects alongside output: files and branches created, labels the fake forge saw change (`$V/ghd/labels-<n>`), forge calls made (`$V/ghd/calls`, `$V/ghd/glab-calls`), phases the fake harness ran (`$V/ctrl/calls`).
- The fakes stand in only at the production boundary `ns` already has: the harness binary and the forge CLI. Everything between runs for real.
- `--dry-run` may still call `gh` (`ns watch --dry-run` and `ns run --dry-run` read the forge). Check `$V/ghd/calls` to see what it did, instead of trusting the flag's name.

Record the feature ID and entry point with each artifact, in `$OUT/README.md` or in `evidence.md`.

## Cleanup

`ns` commands exit on their own. A command started in the background (`ns watch` without `--once`, `ns log -f`) was started as its own group with `setsid "$V/sandbox" <command> & pgid=$!`; kill it with `kill -- -"$pgid"`, per [killing processes](../../../skills/ns-contract/SKILL.md#killing-processes). Then note where the evidence is and remove the sandbox:

```bash
"$V/sandbox" sh -c 'echo "$OUT"; ls "$OUT"'
rm -rf -- "$V"
```

`rm -rf -- "$V"` removes the repo, its worktrees, the fakes and the throwaway `HOME`. Evidence lives in `$OUT`, outside `$V`, so it survives. Cleanup is not a recipe: it runs outside the sandbox it removes.
