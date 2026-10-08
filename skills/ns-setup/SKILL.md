---
name: ns-setup
description: Configure a repo for the nightshift skills by writing its docs/agents/ files (stack, verify, issue tracker, triage labels, domain docs). Use on first use in a repo, or when a skill finds a docs/agents/ file missing.
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
| `docs/agents/triage-labels.md` | label strings for each triage category and state |
| `docs/agents/domain.md` | where `GLOSSARY.md` and ADRs live, and the rules for reading them |

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
- **Test seams**: host unit tests, a simulator or emulator (QEMU, `native_sim`, Renode, a device simulator), and hardware-in-the-loop (a bench runner, a `hil` marker or env, a self-hosted CI runner with a board attached).
- **Worktree traps**: does a fresh `git worktree` build its own sources? Common traps:
  - submodules a new worktree lacks (`.gitmodules`)
  - a shared workspace that resolves the module from the main checkout (west, a monorepo tool, an editable install pointing at the main checkout)
  - generated files or caches outside the tree
  - shared resources only one agent may use at a time (a workspace, a bench)

  Read any worktree notes in `AGENTS.md`. Then prove it: create a scratch worktree, run the build, and check that its outputs name the worktree path (e.g. `compile_commands.json`, `build.ninja`). Remove the scratch worktree afterwards.
- **Verify**: a `verify-<project>` skill in the repo and a control CLI it drives.
- **Tracker**: `git remote -v`. GitHub or GitLab? A `.scratch/` directory means local markdown issues are already in use.
- **Labels**: on GitHub, `gh label list`. On GitLab, `glab label list`.
- **Domain docs**: `GLOSSARY.md`, `GLOSSARY-MAP.md`, `docs/adr/`, and any `*/docs/adr/`.
- **Multi-context signals**: several independently built packages or apps with their own source tree (a uv or Cargo workspace, several firmware apps under one west manifest). Their absence means single-context, which is almost every repo.

Time the fast commands (lint, host unit tests) by running each once. Take slow ones from CI durations.

### 2. Present findings and ask

Summarise what's present and what's missing. Then take the sections in order: one section, one answer, then the next. Lead each with the recommended answer so the user can accept it in a word. Skip a section when exploration settled it.

**Section A: Stack.** Show the drafted commands and seams. Ask only about the gaps: usually the HIL setup (what hardware, how to reach it, how long a run takes), and which command to trust when two disagree. A seam the project lacks is recorded as `none`.

**Section B: Verify.** If a verify skill and control CLI exist, confirm their names. Otherwise record "not set up yet: run ns-setup-verify" without asking.

**Section C: Issue tracker.** Propose GitHub if a remote points at GitHub, GitLab if at GitLab. Otherwise offer:

- **GitHub**: issues live in the repo's GitHub Issues (`gh` CLI)
- **GitLab**: issues live in the repo's GitLab Issues (`glab` CLI)
- **Local markdown**: issues live as files under `.scratch/<feature>/` (solo projects, no remote)
- **Other** (Jira, Linear, etc.): ask for a one-paragraph description and record it as prose

The GitHub and GitLab templates carry a "PRs as a request surface" flag, defaulted **off**. Leave it off and don't raise it.

**Section D: Triage labels.** Ask exactly one question:

> Do you want to keep the default triage labels? (recommended: **yes**)

The defaults are the canonical roles with each label equal to its name (see [triage-labels.md](references/triage-labels.md)). On **no**, usually because the tracker already uses other names, collect the overrides so `ns-triage` applies existing labels instead of creating duplicates.

**Section E: Domain docs.** Default to **single-context** (one `GLOSSARY.md` + `docs/adr/` at the root) and write it without asking. Offer **multi-context** (a root `GLOSSARY-MAP.md` pointing to per-context `GLOSSARY.md` files) only when exploration found multi-context signals.

### 3. Confirm

Show the user a draft of every file from the table above, the `## Agent skills` block, and any labels to create. Let them edit before writing.

### 4. Write

Write `docs/agents/*.md` from the templates in [references/](references/):

- [stack.md](references/stack.md), [verify.md](references/verify.md), [triage-labels.md](references/triage-labels.md), [domain.md](references/domain.md)
- one of [issue-tracker-github.md](references/issue-tracker-github.md), [issue-tracker-gitlab.md](references/issue-tracker-gitlab.md), [issue-tracker-local.md](references/issue-tracker-local.md). For "other", write `docs/agents/issue-tracker.md` from the user's description, covering the same operations.

If the worktree check found traps:
- write the fix-up commands to `.nightshift/nightshift.toml` under `[worktree] setup = [...]`. `ns worktree new` runs them in each new worktree, with `NS_UNIT`, `NS_WORKTREE` and `NS_MAIN_ROOT` set.
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
```

### 5. Done

Done when all five `docs/agents/` files exist with no unfilled placeholders, and the instructions file holds the block. Tell the user which seams are `none` or unmeasured, and, if verify isn't set up, that `ns-setup-verify` is the next step. They can edit `docs/agents/*.md` directly later. Re-run this skill to switch trackers or start over.
