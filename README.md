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

Log in to Claude with your Claude subscription (`claude /login`), and tell `ns` which GitHub account opens the PRs and edits labels. The name is one of the accounts `gh auth status` lists:

```bash
ns config forge github --account <account>
```

It is kept in `~/.config/nightshift/config.toml`, not in the repository. A `GH_TOKEN` already set wins over it.

Then, in each project you want to run the factory on:

1. Run `ns init` and commit what it leaves: `.nightshift/nightshift.toml`, which pins the repository to this version of `ns`, and `.claude/settings.json`, whose hook keeps the skills current in a Claude Code session. It also creates the labels `ns` uses on the forge, if they are missing: the `status:` labels it sets, the `type:` labels (the Conventional Commit types) and the `priority:` labels (`critical`, `high`, `medium`, `low`) that order the queue. In a session, type `/ns-grilling` or any other skill.
2. Put the command that runs the repository's checks (format, lint, tests) in `.nightshift/check`, such as `scripts/ci-local.sh`, and commit it. Without it, a unit's checks pass without running anything.
3. Create the `area:` labels for the parts of your project. `ns triage` only uses labels the repository has, and only you know the areas.

Run `ns doctor` to confirm the setup. It prints a JSON object with one check each for the pin, the harness, the forge login, `.nightshift/check` and the labels, says how to fix any that fails, and exits 1 when one does. It changes nothing.

To move a repository to a newer `ns`, install it, then run `ns upgrade <version>` and commit the new pin.

## Run your first unit

Pick a small issue with clear acceptance criteria, such as a typo or a missing test. Its number is the unit.

```bash
ns triage 12     # decides what it needs next and labels it; a clear one becomes status:ready-for-agent
ns run 12        # runs setup, then stops before build so you can look
```

`ns run` makes the unit's workspace beside the repository (`<repo>.workspaces/12`, on the branch `ns/12`), prints each move as a line of JSON, and keeps the unit's `status:` label up to date. It also assigns the unit and its change request to the account it acts as. Each `ns run 12` runs one state and stops before the next, so the first runs `setup` and stops before `build`; run the same command again to go on from where it stopped, through verify, review and ship. Each state leaves its notes in `.ns/12/` in the workspace. When it is done the change request is open and waiting for you to merge it.

Once you trust the loop, `--gates auto` goes through every state without stopping, and `--merge` lets `ns` merge a change whose checks pass:

```bash
ns run 12 --gates auto --merge
```

A unit that is too vague for `ns triage` to route to build goes to `ns define 12`, a session in which you and the agent work out a spec. `ns run 12 --fresh` throws the workspace away and starts over.

## Run it overnight

```bash
ns auto --until 06:30
```

It triages the issues nobody has looked at, then takes the `status:ready-for-agent` ones one at a time and runs each unattended, starting no new unit after 06:30. A unit already running finishes. A unit that gets stuck is labeled `status:ready-for-human` and `ns auto` moves on; pick it up with `ns run`. If two units in a row get stuck because the harness kept failing, not because their checks did, `ns auto` stops and exits non-zero; `--max-failures-in-a-row` changes the count and `0` turns it off. To try it on a couple of issues first, use `ns auto --max-units 2`, or `--dry-run` to see the queue.

With `--merge`, `ns run` and `ns auto` merge a change request once its checks pass, unless it touches a file matching `[merge] human_review` in `.nightshift/nightshift.toml`. Then it is left ready to merge, labeled `needs-human-merge`, and the unit is `status:ready-for-human`. Without the key the default is `.github/**`, `.nightshift/**`, `release-please-config.json`, `.release-please-manifest.json` and `rust-toolchain.toml`; setting it replaces the default, and an empty list lets every change merge.

```toml
[merge]
human_review = [".github/**", ".nightshift/**", "skills/**"]
```

`ns status` shows where each unit stands in the morning.

`ns eval review` runs the review skill on the known cases in `evals/review` and reports whether it gives the right verdict each time, so you can tell whether an edit to the skill helped. It makes real harness calls, so it uses subscription time.

## Credits

Adapted from [mattpocock/skills](https://github.com/mattpocock/skills), [cursor/plugins pstack](https://github.com/cursor/plugins/tree/main/pstack) and [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills). See [`NOTICE.md`](NOTICE.md).
