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

Each unit of work gets its own git worktree. Phases hand off through files in `.ns/<unit>/` (`brief.md`, `build.md`, `evidence.md`, `review.md`, `pr.md`), so each phase can start in a fresh context. A phase's gate either stops for a human or, under `gates: auto`, moves on by itself. Deploying and releasing always wait for a human. Merging waits for one under `merge.policy = "human"` (the default) or when the PR touches a protected path, and [`docs/FACTORY.md`](docs/FACTORY.md#merge) lists the other cases.

## Install

```bash
cargo install --path cli     # the ns CLI
ns install                   # symlinks skills/ns-* into ~/.agents/skills and ~/.claude/skills
```

Then, in each project you want to run the factory on:

1. `ns-setup`: writes `docs/agents/` (stack, tracker, labels, domain).
2. `ns-setup-verify`: generates the project's verification skill and control CLI.

## Run it overnight

Log in to claude with your Claude subscription, then start `ns watch` as the GitHub account that should open the PRs. `gh` uses whatever `GH_TOKEN` is set, so export it first:

```bash
claude /login
export GH_TOKEN=$(gh auth token --user <account>)
ns watch --until 06:30
```

It takes `status:ready-for-agent` issues one at a time and starts no new unit after 06:30. A unit already running finishes. [`docs/FACTORY.md`](docs/FACTORY.md#ns-watch) lists the other stop conditions, such as `limits.max_units`.

The worktree is not a sandbox. Each phase runs claude with permissions bypassed, so it can use the exported `GH_TOKEN` and read or change anything your user can. Run it on a dedicated machine or VM, with a bot account or a fine-grained token limited to the repo as the only GitHub login on that machine, and mark only issues you trust as ready.

[`docs/FACTORY.md`](docs/FACTORY.md) is the reference for the factory definition (`.nightshift/nightshift.toml`), `ns run` and `ns watch`.

## Skills

| Phase | Skills |
|---|---|
| setup | `ns-setup`, `ns-setup-verify` |
| outer loop | `ns-triage` |
| build | `ns-build`, `ns-tdd` |
| verify | `ns-verify` |
| review | `ns-review`, `ns-no-comments` |
| ship | `ns-ship` |
| shared | `ns-grilling`, `ns-domain-modeling`, `ns-swarm`, `ns-writing-for-agents`, `ns-writing-for-humans`, `ns-principle-*` |

## Credits

Adapted from [mattpocock/skills](https://github.com/mattpocock/skills), [cursor/plugins pstack](https://github.com/cursor/plugins/tree/main/pstack) and [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills). See [`NOTICE.md`](NOTICE.md) and [`docs/SOURCES.md`](docs/SOURCES.md).

## License

MIT
