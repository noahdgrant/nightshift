# nightshift

Agent skills that take an issue to a merged PR: triage, define, plan, build, verify, review and ship. Every phase leaves evidence behind. They work with any agent that reads `SKILL.md` (Claude Code, Codex, Cursor, OpenCode), and with any stack, firmware included.

Status: early. The core path (triage → build → verify → review → ship) is the first slice. See [`docs/DESIGN.md`](docs/DESIGN.md).

## How it fits together

```
 outer loop                     inner loop (the factory)
 issue ─► ns-triage ─┬─────────────────────────► ns-build ─► ns-verify ─► ns-review ─► ns-ship ─► PR
                     ├─ needs-define ─► ns-define ─► ns-plan ─┘
                     └─ needs-repro ──► ns-troubleshoot ──────┘
```

Each unit of work gets its own git worktree. Phases hand off through files in `.ns/<unit>/` (`brief.md`, `build.md`, `evidence.md`, `review.md`, `pr.md`), so each phase can start in a fresh context. A phase's gate either stops for a human or, under `gates: auto`, moves on by itself. Deploying and releasing always wait for a human. Merging waits for one under `merge.policy = "human"` (the default) or when the PR touches a file that needs human review, and [`docs/FACTORY.md`](docs/FACTORY.md#merge) lists the other cases.

## Install

```bash
cargo install --path cli     # the ns CLI
ns install                   # symlinks skills/ns-* into ~/.agents/skills and ~/.claude/skills
```

Then, in each project you want to run the factory on:

1. `ns-setup`: writes `docs/agents/` (stack, verify, tracker, labels, domain, docs).
2. `ns-setup-verify`: generates the project's verification skill and control CLI.
3. `ns-agent-readiness`: audits what in the codebase helps or hurts agents and files the top findings. `ns-setup` offers it.

## Run it overnight

Log in to claude with your Claude subscription, and name the GitHub account that should open the PRs in `~/.config/nightshift/config.toml`:

```toml
[forge.github]
token_command = "gh auth token --user <account>"
```

`ns watch` runs the command once and exports the token as `GH_TOKEN` to `gh` and every phase. A `GH_TOKEN` already set wins. `ns doctor` shows which account resolved.

```bash
claude /login
ns doctor
ns watch --until 06:30
```

It takes `status:ready-for-agent` issues one at a time (`--parallel 3` runs three at once) and starts no new unit after 06:30. A unit already running finishes. [`docs/FACTORY.md`](docs/FACTORY.md#ns-watch) lists the other stop conditions, such as `limits.max_units`.

The worktree is not a sandbox. Each phase runs claude with permissions bypassed, so it can use the `GH_TOKEN` it inherits and read or change anything your user can. Run it on a dedicated machine or VM, with a bot account or a fine-grained token limited to the repo as the only GitHub login on that machine, and mark only issues you trust as ready.

[`docs/FACTORY.md`](docs/FACTORY.md) is the reference for the factory definition (`.nightshift/nightshift.toml`), `ns run` and `ns watch`.

## Skills

| Phase | Skills |
|---|---|
| setup | `ns-setup`, `ns-setup-verify`, `ns-agent-readiness` |
| outer loop | `ns-triage`, `ns-troubleshoot` |
| build | `ns-build`, `ns-tdd` |
| verify | `ns-verify` |
| review | `ns-review`, `ns-no-comments` |
| ship | `ns-ship` |
| all phases, in one session | `ns-auto` |
| shared | `ns-grilling`, `ns-domain-modeling`, `ns-swarm`, `ns-writing-for-agents`, `ns-writing-for-humans`, `ns-principle-*` |

## Credits

Adapted from [mattpocock/skills](https://github.com/mattpocock/skills), [cursor/plugins pstack](https://github.com/cursor/plugins/tree/main/pstack) and [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills). See [`NOTICE.md`](NOTICE.md) and [`docs/SOURCES.md`](docs/SOURCES.md).

## License

MIT
