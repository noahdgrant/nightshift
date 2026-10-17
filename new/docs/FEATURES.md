# Feature inventory

Every feature nightshift has today. Mark each row with a decision in the **Call** column:

- `keep`: part of the rebuilt core
- `later`: cut now, add back when a real need shows up
- `cut`: gone for good

The **Hint** column is my first guess, not a decision. Sizes are rough: words for skills, lines of Rust for the CLI.

## 1. Key abstractions (the seams)

Today only the harness and forge are real seams. The OS and VCS are assumed (Linux/WSL, git).

| Seam | Today | Rebuild target | Call |
|---|---|---|---|
| harness | claude, codex, opencode via `[harness.<name>]` config, plus roles (`ns ask`) | claude only, behind one interface | |
| forge | GitHub (`gh`), GitLab (`glab`) | GitHub only, behind one interface | |
| vcs | git, hard-wired | git only | |
| os | Linux/WSL, hard-wired | Linux only | |
| runner | `systemd` user unit that starts the night, plus fakes for evals; cloud runners are only designed (open design ticket) | one interface, systemd as the only runner | |
| language (rules) | rule packs for rust, python, js/ts, java, c under `rules/`, each with its own tools, tests and baseline, selected by the `rules` row of `docs/agents/stack.md` | one pack interface (install, check, baseline), rust as the only pack |y|

## 2. The loop

| Stage | What it does | Where it lives | Hint | Call |
|---|---|---|---|---|
| build | read the brief, implement test-first in thin slices, write `build.md` | `ns-build` (5k w) | core |y|
| verify | drive the real surface, prove every acceptance criterion, write `evidence.md` | `ns-verify` (6k w) | core |y|
| review | multi-provider review of the diff, fix cycles on findings, write `review.md` | `ns-review` (12k w) | later, start with one reviewer |y|
| ship | open PR, babysit CI to merge-ready, merge | `ns-ship` (8k w) | core |y|
| clean up | save quality data, pull `main`, remove worktree | `ns clean` | core (minus quality data) |y|
| triage | pick next issue, categorise, write the agent brief, route | `ns-triage` (12k w) | core, but much smaller |y|
| define | turn a vague issue into a spec with the human | `ns-triage` route + `ns-requirements` | keep (you decided) |y|
| plan | turn a spec into vertical slices | `ns-triage` route + `ns-design-doc` | keep (you decided) |y|
| repro | get a reliable reproduction | `ns-triage` route | later |y|
| troubleshoot | diagnose a bug with no repro, build a feedback loop, write a brief | `ns-troubleshoot` (5k w) | later |y|
| auto | drive one unit through the phases in one interactive session | `ns-auto` (3k w) | later |y|

## 3. CLI (`ns`, 72k lines)

| Command | What it does | Size | Hint | Call |
|---|---|---|---|---|
| `ns run` | per-unit state machine: worktree, pick next phase, run harness headless, gates, retries, locks, transcripts, run log | 13.8k | core, shrink hard |y|
| `ns watch` | queue loop above `run`: pick ready issues, skip blocked, avoid overlap, parallelism, `--until`, stop on red main, self-update, close split parents | 13.2k | later, start with `--once` |y|
| `ns worktree` | `new`/`remove` a unit's worktree and `ns/<unit>` branch | 0.9k + | core |y|
| `ns clean` | remove worktrees of merged units, save quality records | 0.8k | core |y|
| `ns install` | symlink skills into a harness's skill dir | small | core |y|
| `ns artifact check` | validate `.ns/<unit>/*.md` frontmatter (`ns-artifact/2`) | 0.7k | later |y|
| `ns lint` | lint skills (frontmatter, names, structure) | 1.7k | later |y|
| `ns doctor` | check config, harness binaries, tracker docs | 0.7k + | later |y|
| `ns status` | summarise the night: running, ended, stuck | small | later |y|
| `ns log` | read `runs.jsonl`, phase transcripts, `-f` follow | 0.7k | later |y|
| `ns ask` | run a configured role's harness once (read-only or write) | small | later |y|
| `ns eval` | trigger evals and behaviour cases for skills, `--arms`, `--compare` | 3.1k | later |y|
| `ns score` | grade a finished unit with scorers (rubric + labels + PR outcome) | small | cut? |later|
| `ns quality` | quality records on a `nightshift/quality` branch, finding flow, reviewer stats | 7.5k | cut? |later|
| `ns ratchet` | numbers that may not get worse (binary size, coverage, memory) | 0.4k + | cut? |later|
| `ns rules` | run a repo's rule pack, fail only on findings not in the baseline | 0.5k + | cut? |later|
| `ns check-markers` | check `ns:human-review` markers balance | 0.7k | later |y|
| `ns upstream` | diff adapted skills against their upstream sha | 0.9k | cut? |y|
| `ns transcripts` → `ns learn` (name open) | list and read Claude Code sessions; becomes the input of the session-retro feature (`ns-improve`, open tickets: per-session retros ending in a PR, `ns-supervise` running it at shift end, a tracker retro for structural causes of fix tickets) | 0.7k | fold |this just needs to be folded into the part where we review past sessions to learn|
| `ns factory validate` | validate `nightshift.toml` | 1k | later |y|
| `ns guard-hooks` / hook guard | deny `--no-verify` and other hook bypass inside `ns run` | 0.8k | later |y|

Cross-cutting machinery inside the CLI, each a feature in its own right:

| Feature | Notes | Hint | Call |
|---|---|---|---|
| gates (`stop` / `auto`) | pause for the human or continue between phases | core, simple form |y|
| merge policy + `human_review` globs | auto-merge unless a protected file changed | later |y|
| billing guard | strips API keys so only the subscription login is used | later |y|
| memory cap / cpu limit / disk check | keep parallel units from starving the machine | later (needs `watch`) |y|
| per-phase pinned skills and settings | phases run the skill versions pinned at start (`phase-settings.json`) | later |y|
| per-repo forge accounts | different `gh` account per repo | later |y|
| `pin` (551 lines) | snapshots the skills at the unit's base commit into `.ns/<unit>/skills/` and keeps the phase settings file outside the worktree, so a phase can't edit its own skills or settings | later | |
| `memory` (319 lines) | reads `/proc/meminfo`, creates the systemd `ns.slice` that caps all units, reads each unit's memory peak from its cgroup | later (needs `watch` and the runner seam) | |
| `touches` (278 lines) | reads the globs a brief predicts the unit will change, so `watch` can avoid starting two units that overlap | later (needs `watch`) | |
| `tree` (158 lines) | makes a worktree removable (a phase can leave read-only dirs) before deleting it | fold into `ns worktree remove` | |
| CI-pass cache by tree | `scripts/ci-local.sh` skips a tree that already passed | later |y|
| `systemd` unit + fakes | run the night from systemd | cut? |no this seems pretty reiable for linux. I don't know what the alternative would be. Maybe this becomes a Runner seam|

## 4. Skills (55)

### Phase skills

| Skill | Words | Hint | Call |
|---|---|---|---|
| `ns-contract` | 4.0k | core, shrink: units, artifacts, gates, disclosure line |y|
| `ns-triage` | 11.9k | core, shrink |y|
| `ns-build` | 5.2k | core |y|
| `ns-verify` | 6.5k | core |y|
| `ns-review` | 12.4k | later |y|
| `ns-ship` | 7.8k | core |y|
| `ns-troubleshoot` | 5.4k | later |y|
| `ns-auto` | 3.3k | later |y|
| `ns-supervise` | 2.7k | later (needs `watch`) |y|

### Setup and repo-level

| Skill | Words | Hint | Call |
|---|---|---|---|
| `ns-setup` | 8.8k | core, much smaller: writes `docs/agents/*` |y|
| `ns-setup-verify` | 4.7k | later: generates a `verify-<project>` skill and its control CLI |y|
| `ns-agent-readiness` | 1.9k | later |y|
| `ns-maintain` | 1.7k | cut? only needed while skills are adapted from upstream |this is meant to maintain the verify skill. pstack talks about this|

### Writing and design

| Skill | Words | Hint | Call |
|---|---|---|---|
| `ns-requirements` | 3.8k | keep (define) |y|
| `ns-design-doc` | 9.1k | keep (plan), shrink |y|
| `ns-domain-modeling` | 1.2k | later |y|
| `ns-verify-docs` | 3.7k | later |y|
| `ns-writing-for-agents` | 2.2k | later |y|
| `ns-writing-for-humans` | 1.2k | later |y|
| `ns-grilling` | 0.3k | keep (cheap, useful in define) |y|

### Working methods

| Skill | Words | Hint | Call |
|---|---|---|---|
| `ns-tdd` | 1.7k | core |y|
| `ns-swarm` | 1.0k | later |y|
| `ns-no-comments` | 1.4k | later |y|
| `ns-principles` (index) | 0.9k | later, only if principles stay |y|

### Principles (`ns-principle-*`, 29 skills, about 11k words in all)

Parked: needs a deeper dive later. They come from pstack, so compare against its set before deciding. Most overlap each other. Pick a handful, or fold the survivors into one short `ns-principles` skill.

| Principle | Overlaps | Call |
|---|---|---|
| attack-the-premise | fix-root-causes | |
| boundary-discipline | | |
| build-the-lever | | |
| contract-first | | |
| encode-lessons-in-structure | | |
| exhaust-the-design-space | | |
| experience-first | | |
| explain-the-number | | |
| fix-root-causes | attack-the-premise | |
| foundational-thinking | model-the-domain | |
| guard-the-context-window | | |
| hyrums-law | | |
| keep-scope-tight | laziness-protocol | |
| laziness-protocol | keep-scope-tight, subtract-before-you-add | |
| make-operations-idempotent | | |
| manage-confusion | surface-assumptions | |
| migrate-callers-then-delete-legacy-apis | | |
| minimize-reader-load | | |
| model-the-domain | foundational-thinking, type-system-discipline | |
| never-block-on-the-human | | |
| outcome-oriented-execution | redesign-from-first-principles | |
| pave-the-road | | |
| prove-it-works | | |
| push-back | | |
| redesign-from-first-principles | outcome-oriented-execution | |
| separate-before-serializing-shared-state | | |
| sequence-verifiable-units | | |
| subtract-before-you-add | laziness-protocol | |
| surface-assumptions | manage-confusion | |
| test-behavior-not-implementation | | |
| type-system-discipline | model-the-domain | |

## 5. Rule packs (`rules/`)

Lint rules per language, each with tests, a `TOOLS` pin file and a baseline. Only Rust matters for this repo.

| Pack | Hint | Call |
|---|---|---|
| rust | later | |
| python | cut? |later|
| js/ts | cut? |later|
| java | cut? |later|
| c | cut? |later|
| baseline (`.nightshift/rules-baseline.json`) + `ns rules` | goes with the packs | |

## 6. Docs

| Doc | What it is | Hint | Call |
|---|---|---|---|
| `docs/cli/*` (20 pages) | CLI reference, duplicates `--help` | cut: generate from `--help` |y|
| `docs/factory/*` (8 pages) | run, watch, merge, quality, scorers, unit lifecycle, onboarding, starting a night | cut or fold into one page |y|
| `docs/design/factory-core` | design v1 and its changelog | cut |y|
| `docs/decisions/*` (97 ADRs) | one file per decision | keep only the ones that record a rejected alternative |y|
| `docs/DESIGN.md`, `docs/FACTORY.md` | overall design, factory definition spec | merge into one |no keep as two|
| `docs/EVALS.md` | eval format | later (with `ns eval`) |y|
| `docs/CONVENTIONS.md`, `CODING_STANDARDS.md` | how to write skills and code | keep, short |y|
| `docs/SOURCES.md`, `NOTICE.md` | where adapted skills came from | keep what the licenses need |y|
| `docs/agents/*` (5 files) | per-repo config the skills read: stack, verify, tracker, labels, domain | core |y|
| `README.md`, `AGENTS.md` | entry points | keep, short |y|

## 7. Repo plumbing

| Feature | Hint | Call |
|---|---|---|
| `scripts/ci-local.sh` and `--fast`, affected-file map | later |y|
| `.githooks` pre-commit and pre-push | keep, simple (fmt, clippy, test) |y|
| GitHub Actions `ci.yml` | keep |y|
| `pr-title.yml` (Conventional Commit check) | keep |y|
| `release-please` | later |y|
| `scripts/check-private.sh` and `PRIVATE_DENYLIST` | keep (guards private names) |y|
| `scripts/stress.sh`, flaky-test rules | later |y|
| `scripts/ci-tools.d/` pinned tools | later |y|
| `scripts/check-verify-recipes.sh` | later |y|
| `evals/fixtures` (py-inventory, zephyr-logger, night-log) | later (with `ns eval`) |y|
| label scheme (`type:*`, `area:*`, `status:*`, `priority:*`) | keep a smaller set |no the set is good|
| disclosure line on everything an agent posts | keep |y|
