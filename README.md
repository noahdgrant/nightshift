# software-factory

Agent skills that take an issue to a merged PR: triage, define, plan, build, verify, review and ship. Every phase leaves evidence behind. They work with any agent that reads `SKILL.md` (Claude Code, Codex, Cursor, OpenCode), and with any stack, firmware included.

Status: early. The core path (triage → build → verify → review → ship) is the first slice. See [`docs/DESIGN.md`](docs/DESIGN.md).

## How it fits together

```
 outer loop                     inner loop (the factory)
 issue ─► sf-triage ─┬─────────────────────────► sf-build ─► sf-verify ─► sf-review ─► sf-ship ─► PR
                     ├─ needs-define ─► sf-define ─► sf-plan ─┘
                     └─ needs-repro ──► sf-troubleshoot ──────┘
```

Each unit of work gets its own git worktree. Phases hand off through files in `.sf/<unit>/` (`brief.md`, `build.md`, `evidence.md`, `review.md`, `pr.md`), so each phase can start in a fresh context. A phase's gate either stops for a human or, under `gates: auto`, moves on by itself. Merging, deploying and releasing always wait for a human.

## Install

```bash
cargo install --path cli     # the sf CLI
sf install                   # symlinks skills/sf-* into ~/.agents/skills and ~/.claude/skills
```

Then, in each project you want to run the factory on:

1. `sf-setup`: writes `docs/agents/` (stack, tracker, labels, domain).
2. `sf-setup-verify`: generates the project's verification skill and control CLI.

## Skills

| Phase | Skills |
|---|---|
| setup | `sf-setup`, `sf-setup-verify` |
| outer loop | `sf-triage` |
| build | `sf-build`, `sf-tdd` |
| verify | `sf-verify` |
| review | `sf-review`, `sf-no-comments` |
| ship | `sf-ship` |
| shared | `sf-grilling`, `sf-domain-modeling`, `sf-swarm`, `sf-writing-for-agents`, `sf-writing-for-humans`, `sf-principle-*` |

## Credits

Adapted from [mattpocock/skills](https://github.com/mattpocock/skills), [cursor/plugins pstack](https://github.com/cursor/plugins/tree/main/pstack) and [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills). See [`NOTICE.md`](NOTICE.md) and [`docs/SOURCES.md`](docs/SOURCES.md).

## License

MIT
