---
name: ns-setup
description: Configure a repo for the nightshift skills by writing its docs/agents/ files (stack, verify, issue tracker, triage labels, domain docs, docs locations). Use on first use in a repo, or when a skill finds a docs/agents/ file missing.
metadata:
  upstream:
    - mattpocock/skills@b0618bc436ad:skills/engineering/setup-matt-pocock-skills
---

# Setup

Scaffold the per-repo configuration the `ns-*` skills read:

| File | Holds |
|---|---|
| `docs/agents/stack.md` | languages, build/test/lint/format commands, test seams with command and duration |
| `docs/agents/verify.md` | pointer to the project's verify skill and control CLI |
| `docs/agents/issue-tracker.md` | where issues live, the CLI that reaches them, how they are created and linked |
| `docs/agents/triage-labels.md` | label strings for each triage category, priority and state |
| `docs/agents/domain.md` | where `GLOSSARY.md` and ADRs live, and the rules for reading them |
| `docs/agents/docs.md` | where requirements and design docs live, their review surface, ID keys, docs checks, and template overrides |

This is a prompt-driven skill, not a deterministic script. Explore, present what you found, confirm with the user, then write. The repo answers most questions. Ask only what it can't.

## Process

### 1. Explore

Read whatever exists. Assume nothing:

- `docs/agents/`: does prior output already exist? If so, this run updates it.
- `AGENTS.md` and `CLAUDE.md` at the repo root: which exist, whether `CLAUDE.md` is only `@AGENTS.md`, and whether either has an `## Agent skills` section.
- **Stack signals**, each with the commands it implies:
  - `pyproject.toml` (and `uv.lock`, `poetry.lock`, `tox.ini`, `noxfile.py`): pytest, ruff, mypy and the runner that wraps them
  - `Makefile`, `justfile`: the targets humans actually run
  - `CMakeLists.txt`, `CMakePresets.json`: configure/build presets, `ctest`
  - `west.yml`, `prj.conf`: Zephyr. `west build -b <board>`, `twister`, `native_sim` or QEMU targets
  - `platformio.ini`: `pio run -e <env>`, `pio test -e native` for host tests, on-target envs
  - `Cargo.toml`: `cargo build/test/clippy/fmt`, and `[workspace]` members
  - `.pre-commit-config.yaml`: the lint and format hooks
  - CI workflows (`.github/workflows/`, `.gitlab-ci.yml`): the commands CI runs. Treat these as the ground truth when they disagree with a Makefile or README. CI logs also give run durations.
- **Hooks**: an existing pre-commit hook setup, checked in the order [hooks.md](references/hooks.md) gives (`core.hooksPath`, `.githooks/pre-commit`, `.pre-commit-config.yaml`, `.husky/pre-commit`, `lefthook.yml`, an executable `.git/hooks/pre-commit`), and any script that already runs CI locally.
- **Test seams**: host unit tests, a simulator or emulator (QEMU, `native_sim`, Renode, a device simulator), and hardware-in-the-loop (a bench runner, a `hil` marker or env, a self-hosted CI runner with a board attached).
- **Worktree traps**: does a fresh `git worktree` build its own sources? Common traps:
  - submodules a new worktree lacks (`.gitmodules`)
  - a shared workspace that resolves the module from the main checkout (west, a monorepo tool, an editable install pointing at the main checkout)
  - generated files or caches outside the tree
  - shared resources only one agent may use at a time (a workspace, a bench)

  Read any worktree notes in `AGENTS.md`. Then prove it: create a scratch worktree, run the build, and check that its outputs name the worktree path (e.g. `compile_commands.json`, `build.ninja`). Remove the scratch worktree afterwards.
- **Rule pack**: nightshift ships a pack per language under `rules/`, listed in [rules/README.md](../../rules/README.md). A `Cargo.toml` takes the Rust pack, a Python stack the Python pack, a `package.json` the JavaScript/TypeScript pack, C sources (`*.c`, `*.h`) the [C rule pack](../../rules/c/README.md), and a Maven or Gradle build (`pom.xml`, `build.gradle`, `build.gradle.kts`) the [Java rule pack](../../rules/java/README.md). For C, note whether `ast-grep` and `clang-tidy` are on `PATH`, and how the build writes `compile_commands.json`. For Java, note whether `java -version` reports 11 or later.
- **Verify**: a `verify-<project>` skill in the repo and a control CLI it drives.
- **Tracked skill dirs**: `git ls-files -- .claude/skills .agents/skills`. `ns run` links both paths to each unit's pinned skill snapshot. A tracked `.claude/skills` (a directory or a symlink) stops `ns run` before its first phase, and a tracked `.agents/skills` makes it warn and leave the live copy in place. `ns doctor` reports both.
- **Tracker**: `git remote -v`. GitHub or GitLab? A `.scratch/` directory means local markdown issues are already in use.
- **Labels**: on GitHub, `gh label list`. On GitLab, `glab label list`.
- **Domain docs**: `GLOSSARY.md`, `GLOSSARY-MAP.md`, `docs/adr/`, and any `*/docs/adr/`.
- **Docs**: existing requirements and design docs (`docs/requirements/`, `docs/design/`, `requirements/`, `specs/`), the requirement ID keys they use, and any house template in `docs/agents/templates/`.
- **Multi-context signals**: several independently built packages or apps with their own source tree (a uv or Cargo workspace, several firmware apps under one west manifest). Their absence means single-context, which is almost every repo.

Time the fast commands (lint, host unit tests) by running each once. Take slow ones from CI durations.

A command that starts its own parallel workers (twister, `make -j`, `ctest -j`, `pytest -n`) takes its worker count from `NS_CPUS` when it is set, as in `-j "${NS_CPUS:-$(nproc)}"`. Under `ns watch --parallel`, `NS_CPUS` is each unit's share of the cores, so units don't starve each other's timeout-bound tests. Cargo needs nothing: it reads `CARGO_BUILD_JOBS` and `RUST_TEST_THREADS`, which `ns run` sets beside it.

### 2. Present findings and ask

Summarise what's present and what's missing. If Explore found a tracked `.claude/skills`, raise it first, before any section and before writing anything: say that `ns run` refuses to start in this repo while it is tracked, and propose moving the repo's own skills to `.agents/skills` with `git mv .claude/skills/<skill> .agents/skills/<skill>` for each skill. Run the move only when the user says yes to it. A default or recommended answer is not a yes. Without one, leave the files where they are and add a line to `stack.md`'s Gotchas naming the tracked `.claude/skills` and the move. A tracked `.agents/skills` only needs a mention: `ns run` works there.

Then take the sections in order: one section, one answer, then the next. Lead each with the recommended answer so the user can accept it in a word. Skip a section when exploration settled it.

**Section A: Stack.** Show the drafted commands and seams. Ask only about the gaps: usually the HIL setup (what hardware, how to reach it, how long a run takes), and which command to trust when two disagree. A seam the project lacks is recorded as `none`. On a Python stack, also offer the Python rule pack (recommended: **yes**). It checks every unit for suppressions without a reason, imports outside an allowlist, duplicate helpers and files past 1000 lines. When the repo has a `package.json`, offer the JavaScript/TypeScript rule pack (recommended: **yes**). It runs ESLint, `tsc` when there is a `tsconfig.json`, and ast-grep, from its own `node_modules` under `.nightshift/rules/js/`, and leaves the repo's `package.json` and lockfile alone. When the repo has C sources, offer the C rule pack (recommended: **yes**) and show its `rules` row. When it has a `pom.xml` or `build.gradle(.kts)`, offer the Java rule pack (recommended: **yes**): the same four checks plus nine PMD rules, run on source only, so Maven and Gradle repos install it alike.

**Section B: Hooks.** If Explore found a hook setup, show it as what `stack.md` will record and change nothing. Otherwise propose the pre-commit hook in [hooks.md](references/hooks.md) (recommended: **yes**): `scripts/ci-local.sh` running CI's commands in CI's order, with a `--fast` subset; `.githooks/pre-commit` running `scripts/ci-local.sh --fast`; and `git config core.hooksPath .githooks` for the main checkout and as a `[worktree] setup` line. Show the drafted script, built from this repo's CI. Gates follow [hooks.md](references/hooks.md), Gates.

Then propose the `[merge] human_review` starter list from [hooks.md](references/hooks.md), Merge review list: the CI config, the hook, the local CI script and `.nightshift/**`, those the repo has.

**Section C: Verify.** If a verify skill and control CLI exist, confirm their names. Otherwise record "not set up yet: run ns-setup-verify" without asking.

**Section D: Issue tracker.** Propose GitHub if a remote points at GitHub, GitLab if at GitLab. Otherwise offer:

- **GitHub**: issues live in the repo's GitHub Issues (`gh` CLI)
- **GitLab**: issues live in the repo's GitLab Issues (`glab` CLI)
- **Local markdown**: issues live as files under `.scratch/<feature>/` (solo projects, no remote)
- **Other** (Jira, Linear, etc.): ask for a one-paragraph description and record it as prose

The GitHub and GitLab templates carry a "PRs as a request surface" flag, defaulted **off**. Leave it off and don't raise it.

**Section E: Triage labels.** Ask exactly one question:

> Do you want to keep the default triage labels? (recommended: **yes**)

The defaults are the canonical roles with each label equal to its name, and each priority as `priority:<role>`, the labels `ns watch` sorts on (see [triage-labels.md](references/triage-labels.md)). On **no**, usually because the tracker already uses other names, collect the overrides so `ns-triage` applies existing labels instead of creating duplicates. Custom priority labels also go, highest first, in `[queue] priority` of `.nightshift/nightshift.toml`.

**Section F: Domain docs.** Default to **single-context** (one `GLOSSARY.md` + `docs/adr/` at the root) and write it without asking. Offer **multi-context** (a root `GLOSSARY-MAP.md` pointing to per-context `GLOSSARY.md` files) only when exploration found multi-context signals.

**Section G: Docs.** Propose the directories exploration found (the layout under them is fixed by `ns-requirements`), else the defaults in [docs.md](references/docs.md), with a pull request on this repo as the review surface. Ask only when existing docs live somewhere else or reviewers work in another tool. List any ID keys already in use. Ask for the commands that check the docs (IDs, terms, links), or `none`.

### 3. Confirm

Show the user a draft of every file from the table above, the hook files and `human_review` list from Section B, the `## Agent skills` block, and any labels to create. Let them edit before writing.

### 4. Write

Write `docs/agents/*.md` from the templates in [references/](references/):

- [stack.md](references/stack.md), [verify.md](references/verify.md), [triage-labels.md](references/triage-labels.md), [domain.md](references/domain.md), [docs.md](references/docs.md)
- one of [issue-tracker-github.md](references/issue-tracker-github.md), [issue-tracker-gitlab.md](references/issue-tracker-gitlab.md), [issue-tracker-local.md](references/issue-tracker-local.md). For "other", write `docs/agents/issue-tracker.md` from the user's description, covering the same operations. Every tracker file keeps the template's `## Audit` section with `audit issues per run: 5`.

If the repo has a `Cargo.toml`, install the Rust rule pack, which `ns rules check` runs:

1. Find the pack: `rules/rust/` in the nightshift checkout this skill comes from (`realpath` this skill's directory, then `../../rules/rust`).
2. Run `<pack>/install.sh <repo> <crate-dir>`, where `<crate-dir>` is the directory of the workspace's root `Cargo.toml`, relative to the repo. It copies the pack to `.nightshift/rules/rust/`, writes `allowed-crates.txt` from today's dependency graph, and adds the `rules` row to the Commands table of `docs/agents/stack.md`. Rerunning it refreshes the pack and keeps the repo's lists.
3. Fill `.nightshift/rules/rust/roads.txt` with the helpers that own a job (`<name> <file>` per line: the git wrapper, the subprocess runner), from what Explore found. A second definition of one of those names is a `duplicate-helper` finding.
4. Run `ns rules check --human`. It needs `jq`, `ast-grep` and `cargo-deny` on PATH; a missing one fails naming it. Show the user the findings, and have them accept today's with `ns rules baseline --allow-growth`, a human's act.

If the user took the Python rule pack in Section A, install it as [rules-python.md](references/rules-python.md) says.

If the user took the JavaScript/TypeScript rule pack in Section A, install it as [rules-js.md](references/rules-js.md) says.

If the user took the C rule pack in Section A, install it as its [README](../../rules/c/README.md)'s Install section says: the pack's files under `.nightshift/rules/c/`, the seeded include allowlist and the `rules` row in `stack.md`. Run `ns rules check` and show the user its findings. Accepting them into the baseline is the user's act: give them the `ns rules baseline --allow-growth` command to run.

If the user took the Java rule pack in Section A, install it as [rules-java.md](references/rules-java.md) says.

If no pack was installed, give the `rules` row the command `none`.

If the user took the hook in Section B, write `scripts/ci-local.sh` and `.githooks/pre-commit`, make both executable, and run `git config core.hooksPath .githooks`, as [hooks.md](references/hooks.md) says. Fill `stack.md`'s `ci-local` row and `## Hooks` section either way, from the hook found or the one written. Gates, and where the `[worktree] setup` line goes, follow [hooks.md](references/hooks.md), Gates; if the trap fix-ups below also write a `setup` array, append to it.

Write the `[merge] human_review` list, or give it in the final message, as [hooks.md](references/hooks.md), Merge review list says.

If the worktree check found traps:
- write the fix-up commands to `.nightshift/nightshift.toml` under `[worktree] setup = [...]`. `ns worktree new` runs them in each new worktree, with `NS_UNIT`, `NS_WORKTREE` and `NS_MAIN_ROOT` set, and again when a stop, a crash or a failed command cut them short. Make each safe to rerun on a worktree it already set up.
- write the worktree-correct build and test commands into `stack.md`.
- name any shared resource in `stack.md` under "Gotchas", so phases take turns.

On GitHub or GitLab, create each configured label the tracker lacks (`gh label create` / `glab label create`), with the meaning from the table as its description.

**Pick the instructions file:** edit `AGENTS.md` if it exists. Else edit `CLAUDE.md` if it exists. If neither exists, create `AGENTS.md`. Edit the file that is already there rather than creating its sibling. If an `## Agent skills` section already exists, update it in place and leave surrounding sections untouched.

The block:

```markdown
## Agent skills

Repo settings the `ns-*` skills read. Edit the files directly to change them.

- **Stack**: [languages; host test command]. See `docs/agents/stack.md`.
- **Verify**: [verify skill name, or "not set up yet"]. See `docs/agents/verify.md`.
- **Issue tracker**: [where issues live]. See `docs/agents/issue-tracker.md`.
- **Triage labels**: [default or custom]. See `docs/agents/triage-labels.md`.
- **Domain docs**: [single-context or multi-context]. See `docs/agents/domain.md`.
- **Docs**: [requirements directory; review surface]. See `docs/agents/docs.md`.
```

### 5. Done

Done when all six `docs/agents/` files exist with no unfilled placeholders, the instructions file holds the block, and every rule pack that applies (Rust for a `Cargo.toml`, and each pack the user took in Section A) is installed with `ns rules check` exiting 0, `stack.md` records the repo's hook, found or written, and the `human_review` list is written or given in the final message. Tell the user which seams are `none` or unmeasured, and, if verify isn't set up, that `ns-setup-verify` is the next step. They can edit `docs/agents/*.md` directly later. Re-run this skill to switch trackers or start over.

Then offer an agent-readiness audit: it finds what in the codebase helps or hurts agents and files the top findings as issues. On yes, load the `ns-agent-readiness` skill and run it under `gates: stop`, so the user picks what to file.
