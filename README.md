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

From a release, on Linux (x86_64):

```bash
gh release download --repo noahdgrant/nightshift --pattern '*.tar.gz' --dir /tmp/ns
tar -xzf /tmp/ns/ns-*.tar.gz -C /tmp/ns
install -m 755 /tmp/ns/ns-*/ns ~/.local/bin/ns
```

From a checkout, to run what you are editing:

```bash
cargo install --path cli --force
```

The skills are inside the binary. A repository is pinned to one version of `ns`; see [`docs/VERSIONING.md`](docs/VERSIONING.md).

## Setup

TODO: the lines below about setting up github should be replaced with a `nightshift config forge ...`

Log in to claude with your Claude subscription, and name the GitHub account that should open the PRs in `~/.config/nightshift/config.toml`:

```toml
[forge.github]
token_command = "gh auth token --user <account>"
```

Then, in each project you want to run the factory on, run `ns init` and commit what it leaves: `.nightshift/nightshift.toml`, which pins the repository to this version of `ns`, and `.claude/settings.json`, whose hook keeps the skills current in a Claude Code session. In a session, type `/ns-grilling` or any other skill.

To move a repository to a newer `ns`, install it, then run `ns upgrade <version>` and commit the new pin.

## Run your first unit

TODO: fill this in

## Run it overnight

```bash
ns auto --until 06:30
```

It takes `status:ready-for-agent` issues one at a time (`--parallel 3` runs three at once) and starts no new unit after 06:30. A unit already running finishes.

## Credits

Adapted from [mattpocock/skills](https://github.com/mattpocock/skills), [cursor/plugins pstack](https://github.com/cursor/plugins/tree/main/pstack) and [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills). See [`NOTICE.md`](NOTICE.md).
