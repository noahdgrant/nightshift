# Nightshift

An opinionated software factory for building robust and reliable applications. The factory takes a piece of work through the entire software development lifecycle (SDLC): triage, define, plan, build, verify, review, and ship.

## How it works

```
 outer loop                     inner loop (the factory)
 issue ─► ns-triage ─┬─────────────────────────► ns-build ─► ns-verify ─► ns-review ─► ns-ship ─► PR
                     ├─ needs-define ─► ns-define ─► ns-plan ─┘
                     └─ needs-repro ──► ns-troubleshoot ──────┘
```

## Install

```bash
cargo install --path cli     # the ns CLI
ns install                   # symlinks skills/ns-* into ~/.agents/skills and ~/.claude/skills
ns install --harness cursor  # or into one harness's skill dir: claude, codex, cursor, agents
```

## Setup

TODO: the lines below about setting up github should be replaced with a `nightshift config forge ...`

Log in to claude with your Claude subscription, and name the GitHub account that should open the PRs in `~/.config/nightshift/config.toml`:

```toml
[forge.github]
token_command = "gh auth token --user <account>"
```

Then, in each project you want to run the factory on:

1. `ns-setup`: writes `docs/agents/` (stack, verify, tracker, labels, domain, docs).
2. `ns-setup-verify`: generates the project's verification skill and control CLI.
3. `ns-agent-readiness`: audits what in the codebase helps or hurts agents and files the top findings. `ns-setup` offers it.

[`docs/factory/onboarding-a-repo.md`](docs/factory/onboarding-a-repo.md) walks a Python repo through every step, from `ns install` to its first supervised night.

## Run your first unit

TODO: fill this in

## Run it overnight

```bash
ns watch --until 06:30
```

It takes `status:ready-for-agent` issues one at a time (`--parallel 3` runs three at once) and starts no new unit after 06:30. A unit already running finishes. [`docs/factory/watch.md`](docs/factory/watch.md) lists the other stop conditions, such as `limits.max_units`.

## Credits

Adapted from [mattpocock/skills](https://github.com/mattpocock/skills), [cursor/plugins pstack](https://github.com/cursor/plugins/tree/main/pstack) and [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills). See [`NOTICE.md`](NOTICE.md) and [`docs/SOURCES.md`](docs/SOURCES.md).
