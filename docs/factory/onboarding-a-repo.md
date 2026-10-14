# Onboarding a repo

This page takes a second repo, here a Python one, from `ns install` to its first night of `ns watch`. Do the steps in order: each one reads what the step before it wrote. Each step names the command or skill, what it writes, and how to tell it worked.

The phases run claude with permissions bypassed inside the repo's worktrees. Run the factory on a machine where that is acceptable, as the [README](../../README.md#run-it-overnight) says.

## 1. Install `ns` and the skills

From a nightshift checkout:

```bash
cargo install --path cli
ns install
```

`ns install` symlinks every `skills/ns-*` directory into `~/.agents/skills` and `~/.claude/skills`. Add `--harness codex` or `--harness cursor` for another harness ([`ns install`](../cli/install.md)).

Then log in to claude and name the GitHub account that opens the PRs, as steps 1 and 3 of ["Before the first night"](starting-a-night.md#before-the-first-night) say.

It worked when `ns install` reports no `conflict` and exits 0, and `ns doctor`, run in the new repo, shows `skill_dirs.claude.ns_skills` above 0 and `forge.github.repo_readable: true`.

## 2. Set the repo up with `ns-setup`

Open a Claude Code session in the new repo and ask it to "set this repo up for the nightshift skills", or run `/ns-setup`. The skill reads the repo first, then asks one question per section, each with a recommended answer. It writes:

- `docs/agents/stack.md`, `verify.md`, `issue-tracker.md`, `triage-labels.md`, `domain.md` and `docs.md`
- an `## Agent skills` block in `AGENTS.md`, or in `CLAUDE.md` when only that exists
- the triage labels the tracker lacks, such as `status:ready-for-agent`
- the pre-commit hook, when the repo has none: `scripts/ci-local.sh` with a `--fast` subset built from the repo's CI, `.githooks/pre-commit` running it, and `git config core.hooksPath .githooks`
- the rule pack you accept

Two of these matter most for a night.

**The `ci-local` row.** The `ci-local` row of `stack.md` is the command `ns run` runs as its gate after build ([Gate](run.md#gate)). It names `scripts/ci-local.sh` or the repo's own script that runs CI. Run it once by hand: a gate that fails on today's code sends every unit back to build.

**The Python rule pack.** Say yes when `ns-setup` offers it. It needs `ruff` and `ast-grep` on PATH (`uv tool install ruff ast-grep-cli`). `ns-setup` copies the pack to `.nightshift/rules/python/`, fills its import allowlist from `pyproject.toml`, and adds a `rules` row to `stack.md` ([rules-python.md](../../skills/ns-setup/references/rules-python.md)). `ns rules check --human` then lists today's violations. Accepting them as existing debt is your call, not the skill's:

```bash
ns rules baseline --allow-growth
```

Other stacks get their pack the same way; [`rules/README.md`](../../rules/README.md) lists them.

It worked when the six `docs/agents/` files exist, `scripts/ci-local.sh --fast` (or the repo's own command) passes, and `ns rules check --human` exits 0.

## 3. Write the starter `.nightshift/nightshift.toml`

`ns-setup` proposes `[merge] human_review` in the same run as the hook. It lists the paths a unit must not merge without you: the CI config, the hook directory, the local CI script, and `.nightshift/**` always ([merge.md](merge.md)). Accept it, and `ns-setup` writes it to `.nightshift/nightshift.toml`. Under `gates: auto` it leaves the file alone and gives you the block to add.

A starter file for a Python repo on GitHub with the hook from step 2:

```toml
name = "inventory"
gates = "auto"

[worktree]
setup = ["git config core.hooksPath .githooks"]

[merge]
human_review = [".github/**", ".githooks/**", "scripts/ci-local.sh", ".nightshift/**"]
```

`[worktree] setup` makes every unit worktree run the hook too. The file leaves `[gate] local` unset, which means `every-step`, with the local merge queue ([merge.md](merge.md#local-merge-queue)). Keep it that way on a repo owned by a personal account: `build-only` hands the merge to GitHub's merge queue, which only an organisation-owned repo has. `merge.policy` stays unset, which means `human`: every unit ends at an open PR for you to merge. Turn on `policy = "auto"` only after a few nights of PRs you would have merged as they were. [`docs/FACTORY.md`](../FACTORY.md) lists every other key and its default.

Commit `docs/agents/`, the hook, the rule pack and `.nightshift/` together and push them to the default branch. Each unit's worktree is cut from `origin`, so a phase sees only what you pushed.

It worked when the commit is on `origin`'s default branch and `git symbolic-ref refs/remotes/origin/HEAD` prints it. If that command fails, run `git remote set-head origin --auto`.

## 4. Generate the verify skill with `ns-setup-verify`

In the same kind of session, run `/ns-setup-verify`. It reads `stack.md`, works out how a user drives the project, and writes:

- `verify-<project>`, a skill with a feature map, in the first of `.agents/skills/`, `.claude/skills/` and `.cursor/skills/` the repo has, else in `.agents/skills/`
- `control-<project>`, the CLI that skill calls, by default `tools/control-<project>.py`
- `docs/agents/verify.md`, pointing at both

Keep it under `.agents/skills/`: a `.claude/skills` directory the repo tracks stops `ns run` at start ([Pinned skills and phase settings](run.md#pinned-skills-and-phase-settings)).

It worked when the skill's own launch and doctor steps pass in a fresh session, and `docs/agents/verify.md` names the skill and the control CLI. Commit and push them.

## 5. Check the setup with `ns doctor` and `ns factory validate`

Run both in the repo's main checkout, on the default branch:

```bash
ns doctor
ns factory validate
```

`ns doctor` ([doc](../cli/doctor.md)) checks the config, the harnesses on PATH, the skill directories and the forge account. Its `warnings` don't stop a night, but read them. ["Before the first night"](starting-a-night.md#before-the-first-night) has the fix for the common ones, such as an SSH `origin` with no keepalive.

`ns factory validate` ([doc](../cli/factory.md)) parses `.nightshift/nightshift.toml` and prints each phase's skill, harness, model and timeout. Without the file it fails with `nightshift.toml does not exist; ns run would use the built-in defaults`, which means step 3 didn't land in this checkout. With `policy = "auto"` and an empty `human_review` it warns, and with `[gate] local = "build-only"` on a repo GitHub reports as owned by a personal account.

It worked when both print `"ok": true` and exit 0.

## 6. Run one unit by hand with `ns run --issue N`

Pick a small issue: a bug with a clear repro, or a one-file change. Look at what `ns run` would do first:

```bash
ns run --issue 12 --dry-run
```

The dry run prints the unit id, the first phase (triage, for an issue with no brief) and the prompt it would send. It creates no worktree and calls no harness. Then run it for real and stay at the keyboard:

```bash
ns run --issue 12
```

`ns run` ([doc](../cli/run.md), [spec](run.md)) creates the worktree `../<repo>.worktrees/12-<slug>` on branch `ns/12-<slug>`, and runs triage, build, verify, review and ship. Each phase writes its artifact to `.ns/12-<slug>/` in that worktree. Follow it from another terminal with `ns log -f --unit 12-<slug>`. `ns run` changes no labels.

It worked when the run prints `"outcome": "done"` with a `pr`, and the PR's checks pass. Read the PR as you would a colleague's. A `stuck` outcome names its reason: fix what it says, usually a `stack.md` command or the verify skill, and run it again. Each `ns run` gives the phases fresh attempts.

## 7. Run a short `ns watch`

Mark two or three small issues `status:ready-for-agent`, or run `ns-triage` on them in a session and let it label them (under `ns watch`, the triage pass labels them from its outcome). Check the queue:

```bash
ns watch --dry-run
```

Then run a short night in a terminal, while you are around, with one unit at a time and an end time an hour or two out:

```bash
ns watch --until 16:00 --parallel 1
```

`ns watch` ([doc](../cli/watch.md), [spec](watch.md)) takes the ready issues in order, runs each as `ns run --issue`, and moves the labels: `status:in-progress` while it runs, then `status:in-review` with an open PR, or `status:ready-for-human` with a comment when it is stuck. Before each unit it also triages new issues ([Triage pass](watch.md#triage-pass)). It starts no unit after `--until`, and a unit already running finishes. Ctrl-C stops it and puts the running unit's issue back on the ready label.

It worked when it prints its summary with each unit's `outcome`, and every issue it took carries the label that outcome sets.

## 8. Supervise the first night with `ns-supervise`

Install the systemd timer from ["Install the timer"](starting-a-night.md#install-the-timer), so the night runs in a service of its own. Then, in a Claude Code session in the repo's main checkout, ask it to supervise tonight's night, or run `/ns-supervise`, with the end time and `--parallel`.

The skill starts the night, or attaches to the one the timer started. It pings you when a unit gets stuck, a PR needs your merge, main's CI goes red or the run log goes quiet, and stops the night on a factory-wide failure. It keeps a status report at `<git-common-dir>/ns/handoffs/nightshift-<date>.md` ([Supervising a night](starting-a-night.md#supervising-a-night)). To get each ping outside the session too, set `[supervise] notify` in `nightshift.toml` to a command that reads the ping on stdin, such as `notify-send nightshift`.

In the morning, start from that report and ["In the morning"](starting-a-night.md#in-the-morning). The PRs that wait for you are:

```bash
gh pr list --label needs-human-merge
```

After a few nights of PRs you merge unchanged, raise `--parallel`, and decide whether to turn on `merge.policy = "auto"`.
