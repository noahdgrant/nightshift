# Software Factory: design draft

Status: draft v1, for discussion. Sources: mattpocock/skills, cursor/plugins/pstack, addyosmani/agent-skills.

## Decisions

| # | Decision | Notes |
|---|---|---|
| D1 | A standalone open-source repo. Teams adopt it without forking. | Team-specific settings live in the adopting repo's `docs/agents/` and `.nightshift/`, never in the skills. |
| D2 | Works with any harness. Skills follow the open Agent Skills format (`SKILL.md` + frontmatter) and use no harness-only features. | Harness adapters (plugin manifests, hooks, subagent definitions) are v2. |
| D3 | Each target project carries a verification skill plus a control CLI the agent drives. The factory ships the skill that generates both. | Modelled on pstack's `create-verification-skill` / `maintain-verification-skill` and poteto's "build the lever". |
| D4 | Review uses addyosmani's five axes. | |
| D5 | Model and provider choice belongs to the user. It lives in config, and skills name roles, not models. | |
| D6 | The goal is fully autonomous overnight runs, as in poteto-mode. | Gates have a policy setting: `stop` or `auto`. |
| D7 | One `triage` skill. It adopts Matt's state machine and Agent Brief and adds routing into the inner loop. | See "Triage". |
| D8 | Repo `software-factory`. Skill names use the `sf-` prefix (`sf-define`, `sf-triage`, ...). | |
| D9 | Upstream skills are copied and adapted, not used as submodules. Each copy records its source in `metadata.upstream`. | `sf-maintain` diffs against that. |
| D10 | The `sf` CLI is written in Rust. | Ships as one binary with no runtime, so any harness can call it. Control-CLI templates for target projects stay in the project's own language (Python for the examples). |
| D11 | Agent review runs locally, before push. After push, deterministic CI and any review bots run, and their comments are triaged skeptically. | See "Where review runs". |
| D12 | pstack's principles ship as separate `sf-principle-*` skills. Phase skills point to them by relative path. | They are user-invoked, so the 24 descriptions add no context load. |
| D13 | `sf-swarm` is the shared fan-out leaf. `sf-review` (split by axis) and `sf-verify` (split by feature) use it. | One source of truth for fan-out, aggregation and the PASS/ISSUES/BLOCKED report. |
| D14 | pstack's `unslop` becomes `sf-writing-for-humans`, the define-phase leaf for prose people read. | |

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

## Phases and their artifacts

| Phase | Phase skill | Leaf skills | Input | Output artifact | Gate |
|---|---|---|---|---|---|
| define | `define` | grill-me, grilling, domain-modeling, requirements, design-doc, writing-for-agents, writing-for-humans, design-doc | issue / idea | `SPEC.md`, or an Agent Brief on the issue | human approves the spec |
| plan | `plan` | to-tickets, codebase-design, wayfinder | spec | tickets with blocking edges (tracer bullets) | none by default |
| build | `build` | tdd, implement, prototype | ticket | commits on a branch in a worktree | tests green |
| verify | `verify` | prove-it-works, project verify recipe, the factory CLI | branch | evidence report (commands run, output, HIL/serial logs) | evidence matches acceptance criteria |
| review | `review` | 5-axis reviewers, no-comments, spec review, standards review | diff + spec | findings: Critical / Important / Suggestion | no open Critical |
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
| `needs-define` | intent unclear or the change is large | define |
| `ready-for-agent` | Agent Brief written, small enough to build directly | build |
| `ready-for-human` | brief written, but needs a human (hardware access, a judgement call, credentials) | a person |
| `wontfix` | closed, with an `.out-of-scope/` record | none |

Changes from Matt's version:
- Two new states: `needs-repro` and `needs-define`.
- Under `auto`, triage applies its own recommendation instead of waiting for a maintainer. Comments keep the AI disclaimer.
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

Not every harness can loop on its own. The factory therefore needs a small runner: the `sf` CLI (Rust), which calls the configured harness in headless mode (`claude -p`, `codex exec`, `cursor-agent -p`, ...) once per phase, passes artifacts between runs, and checks gates and the exit condition. This is the factory's own CLI. It is separate from the per-project control CLI.

## Where review runs

Neither pstack nor addyosmani runs agent review in CI. Both review locally, before merge:

- **pstack.** The subagent that opens a PR first runs `interrogate` (a multi-model review panel), `deslop` and `no-comments` on the local diff. After push, Cursor Bugbot and the security reviewer comment on the PR. The Babysit playbook triages those comments skeptically: fix real findings with a failing-first proof, dismiss noise with a disproof. At ship time, one independent verifier per PR posts PASS or FAIL; "CI green is not a verdict."
- **addyosmani.** `/review` and `/ship` review "the staged changes or recent commits" locally, fanning out to the reviewer, security and test personas. CI runs only deterministic gates (lint, types, tests, audit) before "Ready for review", and branch protection requires one human approval.

Our split:

| Where | What | Why |
|---|---|---|
| Local, in the worktree, before push | `sf-review`: the five axes + comments + spec, multi-provider | Fast loop, the full context is there, no noise on the PR, works overnight with nobody watching |
| CI, after push | Deterministic checks only: lint, types, tests, on-host firmware build, HIL if the bench is CI-attached | A check that has to run every time belongs in CI, not in a prompt (retro rule) |
| PR, after push | Review bots (if any) and humans. `sf-ship` babysits and triages their comments | External reviewers catch what a local panel shares blind spots on |

Later, a CI mode of `sf-review` could review PRs from outside contributors, which never had a local review. It runs the same skill headless.

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

Skills say "run the test command from `docs/agents/stack.md`". Inline examples are Python.

## Learning (improve)

Two mechanisms, both ending in a PR a human approves:

1. **Per session (`improve` after a session, or a Stop hook nudge).** pstack `reflect` + Matt's `retro`: reviewers read the transcript and propose changes, sorted mechanically → deterministic check (lint, test, hook), judgement call → `CODING_STANDARDS.md`, workflow → skill edit.
2. **Batch (scheduled weekly).** Mine session transcripts from each harness the user runs. Each harness stores them in its own place (Claude Code `~/.claude/projects/`, Codex `~/.codex/sessions/`, Cursor `agent-transcripts/`), so the factory CLI has one reader per harness. Process only transcripts changed since the last run (incremental index, as in Cursor's continual-learning). Keep patterns seen in two or more sessions: corrections ("no, not that"), repeated manual steps, skills that triggered but got overridden. Output is a PR, never a direct edit.

## Staying current (maintain)

- Each adapted skill records its upstream in frontmatter: `metadata.upstream: mattpocock/skills@<sha>:skills/engineering/tdd`.
- `maintain` diffs upstream HEAD against the recorded sha and opens a PR per skill with the upstream change applied, or a note on why it was skipped.
- Evals guard every skill change: structural lint (frontmatter, links, names), routing (does the description trigger on the right prompts), behavioural (headless `claude -p` on fixture repos, including a firmware fixture).

## Repo layout

```
skills/       one flat dir per skill: sf-<name>/SKILL.md (+ references/, scripts/)
              sf-principle-index.md lists the principles
cli/          the `sf` CLI (Rust): overnight runner, worktrees, transcript readers, skill lint, upstream diff
evals/
docs/
```

## Open questions

See the conversation. Answers get folded back into this file.
