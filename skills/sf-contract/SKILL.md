---
name: sf-contract
description: The software-factory contract that every sf phase skill follows. It covers the docs/agents files, units, worktrees, the .sf artifacts and their frontmatter, gates and gate policy, and evidence. Read it when a phase skill points here.
disable-model-invocation: true
---

# Factory contract

Every `sf-*` phase skill follows these rules. A skill that disagrees with this file is wrong. Fix the skill.

## Target repo configuration

Skills don't name a language, test runner, framework or board. They read the target repo's `docs/agents/` files, which `sf-setup` writes:

| File | Holds |
|---|---|
| `docs/agents/stack.md` | language, build, test, lint and format commands. Test seams: host unit, simulator/emulator, hardware-in-the-loop |
| `docs/agents/verify.md` | pointer to the project's verification skill and control CLI |
| `docs/agents/issue-tracker.md` | tracker, CLI used to reach it, how issues are created and linked |
| `docs/agents/triage-labels.md` | label names for each triage category and state |
| `docs/agents/domain.md` | where the glossary and ADRs live |

If a file is missing, load `sf-setup` instead of guessing.

Code examples are Python. Where firmware changes the advice (registers, ISRs, flash/RAM budgets, on-target tests), add a short firmware note. Don't add a second full example in C.

## Units of work and artifacts

A **unit** is one issue on its way to one PR. Its ID is `<issue-number>-<slug>`, or `<slug>` when there is no issue (`142-uart-timeout`).

Each unit gets one worktree and one artifact folder, created by `sf worktree new <unit-id>`. The first phase that writes an artifact for the unit creates them (`sf-triage` on `ready-for-agent`, `sf-define`, `sf-troubleshoot`, or `sf-build` when started directly). Later phases reuse them, since the command is idempotent:

```
<worktree>/.sf/<unit-id>/
  brief.md       written by sf-triage or sf-define, read by sf-build
  build.md       written by sf-build
  evidence.md    written by sf-verify
  review.md      written by sf-review
  pr.md          written by sf-ship (the PR body as sent)
```

`.sf/` is listed in the repo's `.git/info/exclude`, so artifacts never get committed.

Every artifact opens with frontmatter recording the gate result:

```yaml
---
unit: 142-uart-timeout
phase: verify
status: pass        # pass | fail | blocked
sha: 3f9c2e1        # HEAD commit the artifact describes (build, verify, review, ship)
updated: 2026-10-08T21:14:00Z
---
```

`brief.md` also records `base:`, the branch the unit's worktree came from. Later phases diff against it. A phase may add its own keys (`sf-ship` adds `pr:`).

`sf-ship` requires `evidence.md` and `review.md` to have `sha` equal to HEAD. Review fixes that add commits therefore re-run `sf-verify` before the unit can ship.

- `pass`: the gate condition holds, so the next phase can start.
- `fail`: the gate condition doesn't hold. The body says what is missing. A phase that can fix it itself retries first. Otherwise the phase that owns the fix picks it up, usually the previous one.
- An `inconclusive` check with no failures gives `blocked` when a missing resource caused it, and `fail` otherwise.
- `blocked`: progress needs something an agent can't get (hardware, credentials, a human decision). The body names the blocker.

A phase reads only its input artifact and the repo. It never relies on chat history, so every phase can start in a fresh context.

## Gates

Each phase skill names its gate. What happens at the gate depends on the run's gate policy:

- `gates: stop` (default): stop, report the artifact, and wait for the human.
- `gates: auto`: if `status: pass`, continue to the next phase. Otherwise stop.

The policy comes from the prompt that started the run (`sf-auto` and `sf run` state it). With no policy given, use `stop`.

Some actions always wait for a human, under either policy: force-push to a shared branch, merging, deploying or releasing (including OTA and flashing production units), deleting data, and messaging anyone outside the team.

- A **shared branch** is any branch except the unit's own `sf/<unit-id>`. Force-pushing `sf/<unit-id>` with `--force-with-lease` is fine while every commit on it came from the factory.
- **Messaging outside the team** covers every channel beyond the project's own tracker and PRs (email, chat, customer portals). Comments to external people on the tracker follow the `External comments` setting in `docs/agents/issue-tracker.md` (default `wait`).

## Evidence

Every claim of done carries evidence: the command run, the output that proves it, and where any artifact (log, capture, screenshot) was saved. Something that couldn't be checked is reported as `inconclusive`. It never counts as a pass.
