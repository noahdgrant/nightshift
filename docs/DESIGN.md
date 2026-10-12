# nightshift: design draft

Status: draft v2, for discussion. Sources: mattpocock/skills, cursor/plugins/pstack, addyosmani/agent-skills.

## Decisions

Each decision is one file in [`docs/decisions/`](decisions/). D1 to D67 (the number 40 was never used) were rows of a table here, and keep their IDs as `D<n>-<slug>.md`. A new decision is `docs/decisions/<issue>-<slug>.md` with the ID `ADR-<issue>`, after the issue of the unit that records it; a second decision from one issue is `ADR-<issue>a`, then `b`. Its frontmatter holds `id`, `title` (a one-line summary), `issue`, `date`, and `amends:` or `amended_by:` lists of IDs where they apply, and its body has a Decision and a Reason section. `grep '^title:' docs/decisions/*` lists them.

## Terms

- **Inner loop**: the factory itself. The SDLC phases running on one unit of work.
- **Outer loop**: what feeds the inner loop from outside. Bug reports, new requirements, field failures.
- **Unit of work**: one issue. It enters through the outer loop and leaves as a merged PR.
- **Phase skill**: the top-level skill for one SDLC phase. It delegates to leaf skills.
- **Leaf skill**: does one job (grill-me, tdd, no-comments). It doesn't know which phase it's in.
- **Artifact**: what a phase hands to the next. Phases talk only through artifacts, never through chat history.

## Shape

```
                 OUTER LOOP                                   INNER LOOP (factory)
  issue ──► triage ──┬─ needs define? ── yes ──► define ─► plan ─► build ─► verify ─► review ─► ship ──► PR
                     │                   no ───────────────────────► build
                     └─ bug ──► troubleshoot ──┬─ root cause + small fix ──► build (tdd)
                                               └─ needs redesign ──────────► define

  META LOOP:  improve  (mine past sessions ─► propose skill edits)
              maintain (pull upstream, re-run evals, prune dead skills)
```

`auto` (poteto-mode equivalent) runs the whole chain unattended and pauses only at gates.

## Layers

Five layers run a night. `ns-auto` runs one unit beside them.

```text
 operator       reads the status report; acts in their own sessions
  │ ▲
  │ │ pings when needed; status report at shift end
  ▼ │
 ns-supervise   agent session, one per night (D63): starts or
                attaches to ns watch, watches it (systemd unit, ns status,
                ns log -f, ns quality, gh), pings the operator; may stop
                a night going wrong when the operator said they'd be away
  │ starts or attaches, observes
  ▼
 ns watch       CLI loop for the whole night (systemd timer and service):
                triage pass, picks ready issues by priority, runs up to N
                units in parallel, requeues unfinished units, updates
                itself after cli/ merges, cleans up merged worktrees
  │ one child ns run process per unit
  ▼
 ns run         CLI state machine for one unit (D16), in its own worktree:
                reads .ns/<unit>/ artifacts, starts the next phase's agent,
                runs the CI gate and the merge step (merged under
                merge.policy = "auto", else done and waiting for a human)
  │ a fresh agent session per phase
  ▼
 phase skills   ns-triage → ns-build → ns-verify → ns-review → ns-ship
  ▲
  │ the same skills, chained in one session
 ns-auto        agent session, one unit, interactive: picks each phase
                itself, or hands the loop to ns run; no queue, never merges
```

The operator makes the calls only a person can. `ns-supervise` watches the night and tells the operator when something needs them (D63). `ns watch` owns the queue and the night's limits: `--until`, the budget and `--parallel`. `ns run` owns one unit's state, gates and merge, in code rather than in an agent (D16). The phase skills do the work and hand off only through artifacts. `ns-auto` does the same job as `ns run` from an agent session, with the same state table, and has no queue; [docs/factory/watch.md](factory/watch.md) and [docs/factory/run.md](factory/run.md) cover `ns watch` and `ns run` in full.

## Phases and their artifacts

| Phase | Phase skill | Leaf skills | Input | Output artifact | Gate |
|---|---|---|---|---|---|
| define | `define` | grill-me, grilling, domain-modeling, requirements, design-doc, writing-for-agents, writing-for-humans | issue / idea | `SPEC.md`, or an Agent Brief on the issue | human approves the spec |
| plan | `plan` | to-tickets, codebase-design, wayfinder | spec | tickets with blocking edges (tracer bullets) | none by default |
| build | `build` | tdd, implement, prototype | ticket | commits on a branch in a worktree | tests green |
| verify | `verify` | prove-it-works, project verify recipe, the factory CLI | branch | evidence report (commands run, output, HIL/serial logs) | evidence matches acceptance criteria |
| review | `review` | 5-axis reviewers, no-comments, spec review, standards review | diff + spec | findings: Critical / Important / Suggestion | no open Critical or Important in changed code |
| ship | `ship` | pr, deslop/unslop, changelog | reviewed branch | PR with Why / What / Blast radius / Verification | human merges (or auto per policy) |
| triage | `triage` | grilling, domain-modeling | raw issue | labelled issue + Agent Brief, or needs-info, or wontfix | human confirms category/state |
| troubleshoot | `troubleshoot` | diagnosing-bugs, research | bug report | red-capable repro + root cause + routing decision | repro exists before any fix |
| improve | `improve` | retro, reflect | session transcripts | PR against this repo | human approves each change |
| maintain | `maintain` | upstream sync, evals | upstream repos + this repo | PR against this repo | evals pass |

Rules every phase skill follows:

1. Run in a worktree. The first phase that writes an artifact for the unit creates it, and later phases reuse it (see CONVENTIONS). One worktree per parallel writer.
2. Read its input artifact, write its output artifact, and stop. The next phase can start in a fresh context.
3. Name its gate and either stop there or, under `auto`, check the gate policy.

## Triage

Matt's `triage` is a state machine for issues on a tracker. Each issue gets one category and one state. Its outputs are an Agent Brief for `ready-for-agent`, Triage Notes for `needs-info`, and an `.out-of-scope/` write-up for `wontfix`. Our triage adds a routing question: which phase picks the issue up next. These are the same decision at different detail, so they belong in one skill.

| State | Meaning | Next |
|---|---|---|
| `needs-triage` | new, not yet looked at | triage |
| `needs-info` | the reporter must answer something | wait, then triage |
| `needs-repro` | a bug with no reproducible failure yet | troubleshoot |
| `needs-define` | intent unclear, the change is large, or a design decision is open | define |
| `ready-for-agent` | Agent Brief written, small enough to build directly | build |
| `ready-for-human` | brief written, but needs a human (hardware access, a judgement call, credentials) | a person |
| `wontfix` | closed, with an `.out-of-scope/` record | none |

Changes from Matt's version:
- Two new states: `needs-repro` and `needs-define`.
- Under `auto`, triage applies its own recommendation instead of waiting for a maintainer. Comments open with the disclosure line (`skills/ns-contract`, Disclosure).
- Label names live in the target repo's `docs/agents/triage-labels.md`.

## Verification

"It compiles" is not evidence. Every target project gets two things, generated once by `setup-verify`:

1. **A verification skill** (`verify-<project>`), with Launch, Doctor, Drive, Evidence and Cleanup sections, and a feature map with one file per user-facing feature: how to reach it, how to drive it, what end state proves it, gotchas.
2. **A control CLI** that the skill calls, so agents don't write throwaway scripts. Requirements, from poteto and Cursor's `cli-for-agents`:
   - non-interactive
   - layered `--help` with examples
   - JSON output
   - `--dry-run` on anything destructive
   - idempotent
   - errors that print the correct invocation
   - a `doctor` and a `cleanup` command

   For firmware it wraps the bench: `build`, `flash`, `serial send/expect`, `reset`, `capture` (logic analyzer, power), `hil run <case>`, `doctor` (probe attached, board responds, firmware version matches). For a Python service it wraps launch, requests and logs.

`maintain-verify` re-checks the feature map against source and drives every feature live. It runs on a schedule and opens one PR, or reports a product regression.

## Autonomy (overnight runs)

`auto` needs an overnight contract before it starts:
- the goal
- "done means..." as a checkable condition
- a fresh worktree
- allowed actions
- an escape hatch: what makes it stop and wait

Steps that always wait for a human: force-push, deploy or OTA release, deleting data, messaging people outside the team.

Not every harness can loop on its own. The factory therefore needs a small runner: the `ns` CLI (Rust), which calls the configured harness in headless mode (`claude -p`, `codex exec`, `cursor-agent -p`, ...) once per phase, passes artifacts between runs, and checks gates and the exit condition. This is the factory's own CLI. It is separate from the per-project control CLI.

## Where review runs

Neither pstack nor addyosmani runs agent review in CI. Both review locally, before merge:

- **pstack.** The subagent that opens a PR first runs `interrogate` (a multi-model review panel), `deslop` and `no-comments` on the local diff. After push, Cursor Bugbot and the security reviewer comment on the PR. The Babysit playbook triages those comments skeptically: fix real findings with a failing-first proof, dismiss noise with a disproof. At ship time, one independent verifier per PR posts PASS or FAIL; "CI green is not a verdict."
- **addyosmani.** `/review` and `/ship` review "the staged changes or recent commits" locally, fanning out to the reviewer, security and test personas. CI runs only deterministic gates (lint, types, tests, audit) before "Ready for review", and branch protection requires one human approval.

Our split:

| Where | What | Why |
|---|---|---|
| Local, in the worktree, before push | `ns-review`: the five axes + comments + spec, multi-provider | Fast loop, the full context is there, no noise on the PR, works overnight with nobody watching |
| CI, after push | Deterministic checks only: lint, types, tests, on-host firmware build, HIL if the bench is CI-attached | A check that has to run every time belongs in CI, not in a prompt (retro rule) |
| PR, after push | Review bots (if any) and humans. `ns-ship` babysits and triages their comments | External reviewers catch what a local panel shares blind spots on |

Later, a CI mode of `ns-review` could review PRs from outside contributors, which never had a local review. It runs the same skill headless.

## Review

`review` runs reviewers in parallel as fresh-context subagents, each with the diff and the spec only:

| Reviewer | Source | Notes |
|---|---|---|
| correctness | addyosmani axis 1 | includes the mutation check: flip a condition, the suite must go red |
| readability & simplicity | addyosmani axis 2 | |
| architecture | addyosmani axis 3 | deep-module vocabulary from codebase-design |
| security | addyosmani axis 4 | firmware variant: memory safety, untrusted input over the wire, secrets in flash, debug ports |
| performance | addyosmani axis 5 | firmware variant: ISR latency, stack/heap, flash/RAM budget, power |
| comments | pstack comment-sicko | Python/C suppressions: `# noqa`, `# type: ignore`, `NOLINT`, `#pragma`. Register/errata notes count as the vendor exception |
| spec | mattpocock code-review | does the diff do what the spec asked, no more |

Multi-provider: each reviewer is a role. The user's config maps roles to a harness and a model (D5), and the runner calls that harness's headless CLI in a read-only sandbox. With one provider configured, the reviewers still run, just without the cross-provider check. Findings two providers agree on rank highest (pstack `interrogate`).

## Domain-agnostic by design

Skills never name a language, test runner or hardware. Each target repo carries a `docs/agents/` folder, written once by a `setup` skill:

- `stack.md`: language, build, test, lint commands. Test seams: host unit tests, simulator/QEMU, HIL.
- `verify.md`: how to prove a change on the real surface (CLI, serial console, flashing a board, logic analyzer capture).
- `issue-tracker.md`, `triage-labels.md`, `domain.md`: tracker, label names, and where the glossary and ADRs live.
- `docs.md`: where requirements and design docs live, their review surface and ID keys.

Skills say "run the test command from `docs/agents/stack.md`". Inline examples are Python.

## Learning (improve)

Two mechanisms, both ending in a PR a human approves:

1. **Per session (`improve` after a session, or a Stop hook nudge).** pstack `reflect` + Matt's `retro`: reviewers read the transcript and propose changes, sorted mechanically → deterministic check (lint, test, hook), judgement call → `CODING_STANDARDS.md`, workflow → skill edit.
2. **Batch (scheduled weekly).** Mine session transcripts from each harness the user runs. Each harness stores them in its own place (Claude Code `~/.claude/projects/`, Codex `~/.codex/sessions/`, Cursor `agent-transcripts/`), so the factory CLI has one reader per harness. Process only transcripts changed since the last run (incremental index, as in Cursor's continual-learning). Keep patterns seen in two or more sessions: corrections ("no, not that"), repeated manual steps, skills that triggered but got overridden. Output is a PR, never a direct edit.

## Staying current (maintain)

- Each adapted skill records its upstream in frontmatter: `metadata.upstream: mattpocock/skills@<sha>:skills/engineering/tdd`.
- `ns upstream status` reports which recorded paths changed upstream, and `ns upstream diff <skill>` prints the change (D53).
- `maintain` diffs upstream HEAD against the recorded sha and opens a PR per skill with the upstream change applied, or a note on why it was skipped.
- Evals guard every skill change: structural lint (frontmatter, links, names), routing (does the description trigger on the right prompts), behavioural (headless `claude -p` on fixture repos, including a firmware fixture).

## Repo layout

```
skills/       one flat dir per skill: ns-<name>/SKILL.md (+ references/, scripts/)
              ns-principle-index.md lists the principles
cli/          the `ns` CLI (Rust): overnight runner, worktrees, transcript readers, skill lint, upstream diff
evals/
docs/
```

## Open questions

See the conversation. Answers get folded back into this file.
