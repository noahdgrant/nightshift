# nightshift: design draft

Status: draft v2, for discussion. Sources: mattpocock/skills, cursor/plugins/pstack, addyosmani/agent-skills.

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
| D8 | The project is named **nightshift**. Skills use the `ns-` prefix, the CLI is `ns` (crate `nightshift`), user config lives at `~/.config/nightshift/`, unit branches are `ns/<unit-id>`, and runtime artifacts go in `.ns/<unit-id>/`. | Renamed from software-factory. Other projects named nightshift exist (e.g. marcus/nightshift); kept anyway. |
| D9 | Upstream skills are copied and adapted, not used as submodules. Each copy records its source in `metadata.upstream`. | `ns-maintain` diffs against that. |
| D10 | The `ns` CLI is written in Rust. | Ships as one binary with no runtime, so any harness can call it. Control-CLI templates for target projects stay in the project's own language (Python for the examples). |
| D11 | Agent review runs locally, before push. After push, deterministic CI and any review bots run, and their comments are triaged skeptically. | See "Where review runs". |
| D12 | pstack's principles ship as separate `ns-principle-*` skills. Phase skills point to them by relative path. | They are user-invoked, so the 24 descriptions add no context load. |
| D13 | `ns-swarm` is the shared fan-out leaf. `ns-review` (split by axis) and `ns-verify` (split by feature) use it. | One source of truth for fan-out, aggregation and the PASS/ISSUES/BLOCKED report. |
| D14 | pstack's `unslop` becomes `ns-writing-for-humans`, the define-phase leaf for prose people read. | |
| D15 | Factory as code. A product's factory definition is a directory holding `nightshift.toml`, with `agents/`, `automations/`, `runners/`, `scorers/` and `skills/` beside it. A resource's name is its path. It lives at `.nightshift/` in a product repo, or at the root of its own repo when it spans several repos. Both are supported. | Modelled on Warp Factories' definition files, without promising schema compatibility. The ns-* skills are the library; a definition pins a version of it and adds the product's roles, triggers, runners and scorers. `.nightshift/` is committed; `.ns/` is runtime and excluded. |
| D16 | The foreman is `ns run`, a deterministic state machine in the CLI, not an agent. It reads artifact frontmatter, starts the agent the definition assigns to the next phase, and enforces gates, retries, bench locks and the human-only actions in code. | Agents keep the judgement calls (triage, review). `ns-auto` becomes the in-session form of the same loop. |
| D17 | Triggers run locally first (`ns watch`: polling the tracker plus cron). | Hardware benches sit next to local machines. A GitHub Actions export can come later. |
| D18 | Skills are measured empirically. Each skill carries `evals/` with trigger cases (positive and negative) and behaviour cases (fixture repo, prompt, checks). `ns eval` runs each case several times with and without the skill, or old version against new, across configured harnesses, and reports pass rate, uplift, tokens and time. Online scorers grade real units with the same rubric format and feed `ns-improve`. | From Google's skill evaluation practice and Warp's scorers. Fixture repos: one Python, one firmware (host build or emulator). Deterministic graders over `.ns/` artifacts where possible, LLM judge with a rubric otherwise. A skill whose ablation shows no uplift is a retirement candidate. |
| D19 | `ns-define` routes by size: a grilled Agent Brief for small work, a spec for a feature, and `ns-requirements` then `ns-design-doc` for a new product or subsystem. `ns-verify-docs` verifies doc units. Requirement IDs thread through design, tickets, tests and `evidence.md`. Approving requirements or a design is human-only. | Locations, review surface and templates come from the target repo (`docs/agents/docs.md`, overridable templates), so a team's house format stays in its own repos. |
| D20 | nightshift merges its own PRs when review passes, CI is green and no file that needs human review changed (`merge.policy = auto`). Files that need human review cover the factory's own guardrails and still need a human merge. | Only `ns run`'s merge step merges, only its own unit's PR, only by squash. A PR merged by a phase stops the run. Spec: `docs/FACTORY.md`. |
| D21 | A file can fence part of itself for human review with `ns:human-review` start and end lines (start takes an optional reason). The merge step asks for a human merge when a PR changes a line inside a region in either version, or changes a marker line. | Path globs guard whole files, so a big file with one safety-critical table needed a human merge for every change. Substring detection works in any comment syntax. `ns check-markers` rejects unbalanced or nested markers. Spec: `docs/FACTORY.md`. |
| D22 | An artifact stays current across a rebase: its `sha` is current when it is HEAD or when the unit's diff against the base has the same `git patch-id` as HEAD's. `ns run`'s state table, archiving and merge step share the one check. | A rebase onto a newer main used to end a reviewed unit as "needs a human merge" (#69). A diff that changed after review sends the unit back through verify, review and ship, bounded by `max_attempts`. Two choices go beyond the issue's literal text: the merge matches the PR head (including the pre-update head after `BEHIND`), and the base defaults to the checked-out branch, never the literal `HEAD`. `git patch-id` ignores whitespace, so a whitespace-only change after review stays current; whitespace-significant files (Makefiles, Python, YAML) can change behavior that way unreviewed. |
| D23 | A runner (`runners/<name>.toml`) names exclusive locks, and a phase that sets `runner =` holds them for the whole phase. A run that finds a lock held waits for it; it doesn't refuse. | Overnight, a bench or Zephyr workspace shared by two repos' `ns watch` should queue the second unit, not end its night with exit 5. The locks are OS `flock`s, so a dead run's hold goes with its process. Taken in sorted order, so two runs can't deadlock. Amended by D33. Spec: `docs/FACTORY.md`. |
| D24 | `ns-review` scales its panel and its mutation runs with the change, and a fix cycle re-asks only the axes that still have findings, plus correctness. Every open Critical and Important finding is still fixed on every axis within the 3-cycle limit. | Review took 49% of overnight phase time (#109): eight reviewers on every pass, and up to five full-suite runs per pass. The saving comes from not re-asking axes that already passed. Deliberate choices: a diff under `skills/`, `AGENTS.md`, `CLAUDE.md` or `docs/agents/` is agent-facing instructions and gets the full panel; a reviewer whose findings were all dismissed is not re-run; each mutation gets a timeout of three times the listed test time (at least 60 seconds) and an inconclusive one reruns once at double. No eval case yet checks that a large diff still gets the full panel (it needs a fixture), and the fix-loop, later-pass and timeout rules need multi-cycle model runs; both are tracked with #41. Spec: skills/ns-review/SKILL.md. |
| D25 | `ns-review` writes `blocked`, not `fail`, when a Critical finding is still open after the third fix cycle. `fail` is left for fix-cycle commits that leave the tests or the verify re-run failing. | A `fail` sent the unit back to build, which spent an attempt and usually ended in the same loop (#110). Three fix cycles that could not close a Critical mean a human has to choose: fix it by hand (then delete `review.md` so `ns run` reviews again), split the issue, or send it to `ns-define`. `ns run` already stops on any `blocked` artifact and `ns watch` quotes its first line, so the change is in the skill only. Cargo tests pin the CLI half (blocked stops, fail routes to build); only the model-run eval case `critical-after-three-cycles-is-blocked` checks the skill's rule, and the `fail` half and the multi-cycle loop need multi-cycle model runs tracked with #41. Spec: skills/ns-review/SKILL.md. |
| D26 | Units have a size budget in changed lines, set in `docs/agents/stack.md` as `unit size: soft 400, hard 800` (those defaults when absent). `ns-triage` records a size estimate in every brief and splits work past the soft limit, or holding more than one shippable behaviour, into child issues with `Blocked by` edges; the parent goes to `needs-define` (labels untouched while `status:in-progress`, where `brief.md` gets `status: blocked`) and nothing closes. `ns-build` writes `blocked` ("unit too large, split it") when its diff passes the hard limit before the criteria are met. | Large units made review slow and fix loops long (#116). Splitting at triage is cheaper than discovering size in review. The parent stays open so a human sees the split; under `gates: auto` the children go straight to `ready-for-agent` with the parent's priority. A blocked build is re-triaged by a human. The unit-size line is parsed by the model; no CLI code backs it. The limits are defined once in `skills/ns-contract/SKILL.md`. Triage sizing has eval cases for the split (`broad-feature-split`) and for the paths that don't split (`small-feature-no-split`, and the Size line in `bug-ready-for-agent`); the build stop needs a model run that writes past 800 lines and has none. |
| D27 | After the third fix cycle, an open Important finding in code the unit changed blocks the unit like an open Critical: `review.md` is `blocked` and its first line names them (`Open after 3 fix cycles: C1, I2.`). An open Important in pre-existing code still becomes a follow-up issue, and Suggestions never block. The `ns watch` stuck comment lists a blocked review's open Critical and Important findings. | Deferring a changed-line Important merged code below the bar, and the debt compounded: the 2026-10-09 baseline was about 6 Important findings per unit (#149). Pre-existing code is not the unit's to fix, so it can't hold the unit back. Listing the findings in the comment lets a human finish them, or re-scope the issue, without opening the worktree. The CLI already stops on any `blocked` artifact; it now also names that artifact when a run stops on one left by an earlier run, so the comment can list its findings. `ns-ship`'s GO check holds the same line. The model-run eval case `important-after-three-cycles-is-blocked` checks the blocking rule; a case showing that a pre-existing Important doesn't block comes with #148, which changes how those findings are filed. Spec: skills/ns-review/SKILL.md. Amended by D29. |
| D28 | Review's quality rules have one checklist, `skills/ns-contract/references/quality-bar.md`, one section per review axis. `ns-build` checks its own diff against it before writing `build.md`: every bar item marked, a test for every branch and error path it adds, and a targeted mutation pass using review's mutation procedure, all recorded under `## Self-check`. Reviewers get their axis's section in their brief. `ns-tdd` points to the same test-completeness rule. | Every rule review enforced lived only in `ns-review/references/`, so build never had a chance to meet it, and review found about 6 Important findings per unit (#150). The bar lists what must hold; each axis reference still says how to hunt for violations, so the upstream-derived reference files keep their structure. It lives in `ns-contract` because both phases read it. Each bar item carries its severity, and those tags win over the axis references, so a style nit can't block a unit under D27. The model-run eval case `py-self-check-before-handoff` checks the self-check by running the trial's own tests against a boundary mutant and a nothing-to-cancel mutant. `ns-review` and `ns-tdd` get no new case: their change is a pointer to the bar, and their existing cases now install it. Measuring the effect on first-pass yield needs `ns quality` (#147) and is tracked in #159. Spec: skills/ns-build/SKILL.md. |
| D29 | Amends D27. `ns-review` scopes every finding when it judges it: `changed` (a line the diff adds, a behaviour it changes or removes, or something it leaves out) or `pre-existing`. A pre-existing finding is an **escape**: whatever its severity, including Critical, it never blocks the unit and never goes to the fix loop. Review files it as soon as it is judged, as a `needs-triage` issue with a category and area label, a priority from severity (Critical or any security finding `critical`, falling back to `high` when the tracker has no `critical` role; Important `high`; Suggestion `low`), a link to the unit and the words "escaped an earlier review", and marks it `deferred: #<n>`. Changed-code Criticals and Importants block at the cycle limit as D27 says. Every finding in `review.md` and `cycle-<n>.md` carries `Axis`, `Scope` and `Cycle` fields, so `ns quality` (#147) can count escapes. | D27 still blocked on a Critical in code the unit didn't touch, which held a unit back for a defect it neither caused nor owned, and filed only pre-existing Importants, so a Critical or Suggestion found outside the diff had no tracked home (#148). An escape means an earlier review missed something, so it gets an issue at an urgency matching its severity, and the metric that counts escapes needs a parseable field rather than prose. Filing at judgment time, not at the cycle limit, means a unit that passes on its first review still files what it saw. Reviewers are told to report pre-existing defects they notice, and the Noted bucket no longer dismisses a real defect as "outside the diff". The model-run eval case `pre-existing-defect-is-filed-not-blocking` checks a clean diff beside a seeded pre-existing off-by-one: review passes, the finding is `pre-existing` and `deferred`, the local tracker gets the issue, and the defect stays unfixed. Spec: skills/ns-review/SKILL.md. |
| D30 | `ns quality` measures review outcomes from the artifacts already on disk: first-pass yield, first-pass Critical and Important findings per 100 changed lines by axis, cycles to clean, leftovers at the cycle limit, and escapes, with a trend by local day. It parses `review.md` leniently and reports what it couldn't parse instead of dropping it. An escape is D29's `pre-existing` finding: it is counted and blamed (`git blame` at the reviewed commit, where the finding's line number holds) to the PR that introduced the line, and like D29 it never counts against the unit, so it is left out of first-pass yield, findings per 100 lines, cycles to clean and leftovers. | Without a number we couldn't tell whether build was getting better at passing review (#147). Reading the artifacts needs no new logging, and the run log adds review cost and outcome. Most existing artifacts predate D29's `Axis`, `Scope` and `Cycle` fields, so the command falls back to `Raised by`, `fixed (cycle 1, ...)` and the first pass's cycle file, and counts a unit with fix cycles as dirty when nothing says which pass raised a finding. Cycle-file ids are not matched against `review.md`: old artifacts reuse ids across passes. First-pass numbers use a unit's oldest review artifact; dismissed findings count toward nothing. The totals are higher than the issue's hand-counted baseline (357 findings against 214 on the same 25 units): a heading such as `### I1-I11 (cycle 1)` counts as eleven findings, and one-line `- S1. ...` suggestions count too, where the baseline counted only `###` headings. The `review.md` parser is shared with the `ns watch` stuck comment, which now also lists open findings written in those older shapes. Spec: `docs/FACTORY.md` (Quality). |
| D31 | Before each unit, `ns watch` runs a triage pass: the triage phase alone, once each, on open team-authored issues that carry `[queue] triage_label` (`status:needs-triage`) or no state label, best priority first, up to `[queue] triage_per_night` runs a night (default 10). A triage-only run never sets `in_progress_label`, so `ns-triage` applies the state and priority itself; its prompt says `Triage only` and the skill stops there, with no hand-off. An issue is tried once a night, whatever came of it. Triage-only runs share the night's budget, `--until`, usage-limit sleeps and harness breaker with units, but are not units for `--once` and `max_units`. A run that can't start is recorded and skipped. Under `gates = "stop"` the pass is off. | Review files escapes and follow-ups as `needs-triage` issues (D29), and only ready issues were queued, so tech debt piled up untriaged (#98). Triaging before each pick, not once at the start, lets an escape filed by tonight's first unit be triaged and, when ready, built the same night, sorted in by its priority. Running the pass first spends the night on the debt the operator cares most about clearing; the cap stops a flood of issues from eating the night. #98 asked only for unlabelled issues nightshift filed; the pass takes every unlabelled team issue, since GitHub can't tell which a phase filed and an unlabelled team issue is untriaged work either way, and the cap bounds the cost. Trying each issue once a night stops a triage that leaves an issue untriaged (needs a human, or a failed run) from looping. Leaving `in_progress_label` off keeps the split in `docs/agents/triage-labels.md`: skills set the states, `ns watch` only its run states. Spec: docs/FACTORY.md ("Triage pass"). |
| D32 | `ns-build` marks each open risk in `build.md` as `file:` (a concrete defect or missing work outside the unit's scope) or `note:` (about this unit only). `ns-ship` files the `file:` ones after its GO call, before it writes the PR body: the same duplicate search, `needs-triage` state, category and area labels and unit link as a D29 escape, with the words "found while building <unit-id>, outside its scope" and no priority. It rewrites each line to `filed: #<n>`, or `unfiled: <reason>` when the tracker can't be reached, which never holds the unit back, rewriting each line as soon as its issue exists; a re-run files `unfiled:` lines again and skips `filed:` ones, and an unprefixed line counts as `note:`. It lists them under the PR's Follow-ups. | Build's open risks were the last out-of-scope findings nothing filed, so they died in `build.md` (#168). Ship files them, not build, because a unit that never merges would otherwise leave issues behind for a defect nobody has confirmed past build, and a build retried after `fail` would file again; by GO the unit has passed verify and review, and the risk has been seen by both. The cost: a unit that stops before ship leaves its `file:` risks unfiled in `build.md`, where a human reading the blocked unit still sees them. Priority is left to triage, since a build risk has no review severity to map from. The model-run eval case `py-out-of-scope-defect-marked-for-filing` is a build case: a low-stock report whose obvious implementation reuses `stock_levels`, which counts expired reservations; build must leave `stock_levels` unchanged, mark the defect `file:`, and file nothing itself. Ship has no eval cases yet (it needs a forge to open a PR against), so the filing step itself has none. Spec: skills/ns-ship/SKILL.md. |
| D33 | Amends D23. A run waiting for a runner lock retries it non-blocking with a backoff of up to 5 s on the run's clock, and gives up at its own phase timeout, counted from when the wait began, or at `ns watch --until`, whichever is first. Giving up releases the locks already taken and logs `lock_wait_timeout`. The phase timeout counts as a failed attempt of that phase; `--until` ends the run as `budget`, and `ns watch` reports the night stopped by `until`. A lock taken at or past `--until` is given up the same way, and a budget spent by the time the locks are held ends the run as `budget` with the locks released, before the harness starts. | A blocking `flock` had no bound: a hung holder, or a harness orphaned by a killed `ns`, kept the waiter blocked past its own timeout and past `--until` (#99). Polling on the `Clock` lets `NS_NOW` drive the wait in tests without real sleeping. Reusing `budget` for `--until` needs no new outcome or exit code: `ns watch` already returns the issue to the ready label and stops on `budget`, and names the stop `until` when the deadline has passed. A timeout as a failed attempt matches a timed-out harness (#117), and the retry gives a briefly stuck holder another chance. The budget is checked again once the locks are held, so the harness never starts on a spent budget, whatever the wait did to the clock or the spend. Spec: `docs/FACTORY.md` ("Runners"). |
| D34 | Amends D26. A triage split files at most 5 children. Past that it files none, posts Triage Notes saying why, and moves the parent to `needs-define` (`needs-info` when the reporter must answer first); under `status:in-progress`, `brief.md` gets `status: blocked`. Children of a parent authored outside the team stay `needs-triage` under any gate policy. Each child is recorded in the parent's Triage Notes as soon as it is filed, and a rerun reuses the listed children and files only the missing ones. `Blocked by: #N` names only a sibling from the split or an existing issue looked up on the tracker, never a number from the reporter's text. | D26 put no bound on a split and sent every child to `ready-for-agent` under `gates: auto`, so an outside reporter could steer the factory into filing and building any number of issues, and a run killed midway filed duplicates on rerun (#133). Five children is about what a human can check in one sitting; more means the request needs a human to reshape it, not a bigger split. An outside reporter's request gets a team member's look before building, the same line D31 draws for the triage pass. The model-run eval cases `over-cap-split-asks-human` (eight independent commands, no children filed) and `external-author-split-needs-triage` (the `broad-feature-split` request from a customer, children stay `needs-triage`) cover the cap and the outside-reporter rule. Rerun safety and the blocker source have no case: they need a half-finished prior run and a tracker to look issues up on. Spec: skills/ns-triage/SKILL.md. |

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
