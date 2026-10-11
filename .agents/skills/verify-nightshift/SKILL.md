---
name: verify-nightshift
description: Verify nightshift's `ns` CLI and skills on the real surface by driving the built `ns` against a throwaway git repo, with fake `claude`, `gh` and `glab` on PATH. Use when proving a change to `cli/` or `skills/` works the way a user runs it, or when ns-verify asks for this repo's verify skill.
---

# Verify nightshift

The product is the `ns` CLI and the `ns-*` skills. The control CLI is `ns` itself: there is no separate control script. Every drive runs the `ns` built from the worktree under test, inside a sandbox directory that holds a throwaway git repo, a bare `origin`, a throwaway `HOME`, and the fake `claude`, `gh` and `glab` from `cli/tests/fixtures/factory/`.

Never call a real model or the real GitHub or GitLab. Doctor checks that the fakes are the binaries on PATH before anything that would start a harness or reach a forge.

Read [features/README.md](features/README.md) before driving, then use the feature file that matches the change.

## Launch

`ns` is a short-lived CLI, so launch means build once and set up the sandbox. Run from the root of the worktree under test:

```bash
cargo build --manifest-path cli/Cargo.toml
export NS_SRC=$PWD NS=$PWD/cli/target/debug/ns
export V=$(mktemp -d /tmp/ns-verify.XXXXXX)
mkdir -p "$V/bin" "$V/home/.claude" "$V/ctrl" "$V/ghd"
cp cli/tests/fixtures/factory/{claude,gh,glab} "$V/bin/"
echo '{}' > "$V/home/.claude/.credentials.json"
: > "$V/ghd/issues.jsonl"
echo '[ "$1 $2" = "api user" ] && { echo octocat; exit 0; }' > "$V/ghd/hook.sh"
cat > "$V/env.sh" <<EOF
export PATH="$V/bin:\$PATH" HOME="$V/home" NS_CONFIG="$V/config.toml" FAKE_CTRL="$V/ctrl" FAKE_GH_DIR="$V/ghd" TZ=UTC
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
unset GH_TOKEN GITLAB_TOKEN ANTHROPIC_API_KEY XDG_CONFIG_HOME
EOF
. "$V/env.sh"
printf '[roles.default]\nharness = "claude"\n[forge.github]\ntoken_command = "echo fake-token"\n' > "$NS_CONFIG"
git init -q --bare -b main "$V/remote.git"
git init -q -b main "$V/repo"
echo hi > "$V/repo/README"
git -C "$V/repo" add . && git -C "$V/repo" commit -qm init
git -C "$V/repo" remote add origin "$V/remote.git"
git -C "$V/repo" push -q origin main && git -C "$V/repo" remote set-head origin main
export OUT=${OUT:-$NS_SRC/.ns/verify/$(date -u +%Y%m%dT%H%M%SZ)}
mkdir -p "$OUT"
printf 'export NS_SRC=%q NS=%q V=%q OUT=%q\n' "$NS_SRC" "$NS" "$V" "$OUT" >> "$V/env.sh"
cat >> "$V/env.sh" <<'EOF'
cap() { n=$1; shift; printf '$ %s\n' "$*" > "$OUT/$n.cmd"; "$@" > "$OUT/$n.out" 2> "$OUT/$n.err"; echo $? > "$OUT/$n.exit"; cat "$OUT/$n.exit"; }
EOF
. "$V/env.sh"
echo "$V"
```

Ready when `cargo build` finished and `git -C "$V/repo" log --oneline` shows `init`. Note the printed `$V`. Every later shell runs `. <that dir>/env.sh` first: it restores `NS_SRC`, `NS`, `V`, `OUT` and the `cap` helper. A shell without it would reach the real `claude`, `gh` and `HOME`.

`ns worktree` puts unit worktrees beside the repo, in `$V/repo.worktrees/`, so they stay inside the sandbox. Each run gets its own `$V`, so two runs can go side by side.

## Doctor

Run before the first drive and whenever anything looks off:

```bash
. "$V/env.sh"
for b in claude gh glab; do case "$(command -v $b)" in "$V/bin/"*) ;; *) echo "NOT FAKE: $b"; esac; done
"$NS" --version && git -C "$NS_SRC" rev-parse --short HEAD
"$NS" doctor | jq -c '{ok, problems, forge: .forge.github, claude: .harnesses.claude.path}'
```

Worth driving when no `NOT FAKE` line prints, `ok` is `true`, `problems` is `[]`, `forge` shows `"account":"octocat"`, and `claude` is `$V/bin/claude`. Stop on any `NOT FAKE` line: the next `ns run` or `ns watch` would start a real model or call a real forge.

## Drive

Drive from `$V/repo` for commands that act on a target repo (worktree, ask, factory, run, watch), and from `$NS_SRC` for commands that act on this repo's skills (lint, eval, install). Capture each drive with the `cap` helper from Launch.

```bash
. "$V/env.sh"; cd "$V/repo"
cap worktree-new "$NS" worktree new 7-fix-the-thing
cap worktree-list "$NS" worktree list
cd "$NS_SRC"
cap lint "$NS" lint skills
cap eval-dry "$NS" eval ns-tdd --dry-run
```

Seed the fake forge before `ns run` or `ns watch`. The fakes read files under `$FAKE_GH_DIR`; the header of each fake in `cli/tests/fixtures/factory/` lists them. An open, ready GitHub issue #7:

```bash
echo '{"number":7,"title":"Fix the thing","url":"https://github.com/o/r/issues/7","state":"OPEN"}' > "$V/ghd/issue-7.json"
echo '{"number":7,"title":"Fix the thing","labels":[{"name":"status:ready-for-agent"}],"body":"","authorAssociation":"MEMBER"}' >> "$V/ghd/issues.jsonl"
echo status:ready-for-agent > "$V/ghd/labels-7"
```

The fake `claude` writes each phase's artifact with status `pass` unless `$FAKE_CTRL/<phase>` queues another action (`fail`, `blocked`, `limit`, ...). Its header lists them. It records each phase in `$FAKE_CTRL/calls` and each prompt in `$FAKE_CTRL/prompt-<n>-<phase>.txt`.

The feature files hold the recipe for each command group.

## Evidence

Evidence goes in `$OUT`: `.ns/verify/<run-id>/` under `$NS_SRC` by default. To use the directory `ns-verify` names (`.ns/<unit-id>/evidence/`), export `OUT` before Launch.

`cap <name> <command...>` saves the command, stdout, stderr and exit code as `$OUT/<name>.{cmd,out,err,exit}`. Proof standards:

- Drive the user path: the `ns` command a user types, against a git repo. Calling a Rust function or a test helper is not proof.
- Capture the action and the resulting state. After a command that writes, save a second, read-only view: `ns worktree list`, `git -C "$V/repo" branch -a`, `ls` of the artifact folder, `$V/ghd/calls`, `$V/ghd/labels-<n>`.
- Verify side effects alongside output: files and branches created, labels the fake forge saw change (`$V/ghd/labels-<n>`), forge calls made (`$V/ghd/calls`, `$V/ghd/glab-calls`), phases the fake harness ran (`$FAKE_CTRL/calls`).
- The fakes stand in only at the production boundary `ns` already has: the harness binary and the forge CLI. Everything between runs for real.
- `--dry-run` may still call `gh` (`ns watch --dry-run` and `ns run --dry-run` read the forge). Check `$V/ghd/calls` to see what it did, instead of trusting the flag's name.

Record the feature ID and entry point with each artifact, in `$OUT/README.md` or in `evidence.md`.

## Cleanup

`ns` commands exit on their own. A command started in the background (`ns watch` without `--once`, `ns log -f`) was started as its own group with `setsid cmd & pgid=$!`; kill it with `kill -- -"$pgid"`, per [killing processes](../../../skills/ns-contract/SKILL.md#killing-processes). Then remove the sandbox:

```bash
rm -rf "$V"
ls "$OUT"
```

`rm -rf "$V"` removes the repo, its worktrees, the fakes and the throwaway `HOME`. Evidence lives in `$OUT`, outside `$V`, so it survives; `ls "$OUT"` proves it.
