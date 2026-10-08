---
name: ns-setup-verify
description: Generate a project's verification skill (verify-<project>) and the control CLI it drives, then prove both end to end. Use when a repo has no scripted way to drive its real surface (CLI, service, board, emulator), when docs/agents/verify.md is missing or points nowhere, or when ns-verify asks for it.
metadata:
  upstream:
    - cursor/plugins@ccb5507cec15:pstack/skills/create-verification-skill
    - cursor/plugins@ccb5507cec15:cli-for-agent/skills/cli-for-agents
    - cursor/plugins@ccb5507cec15:cursor-team-kit/skills/control-cli
---

# Set up verification

Every target project needs a scripted way to drive the real thing and prove behavior: launch it, exercise a feature the way a user would, and capture evidence. This skill generates two pieces of infrastructure in the target repo:

1. A **verification skill**, `verify-<project>`, with a feature map.
2. A **control CLI**, `control-<project>`, that the skill calls so agents stop writing throwaway scripts.

You write both for the next agent, not for a human. It will read them cold, mid-task, with no knowledge of the project.

## 1. Interview the repo, not the user

Read `docs/agents/stack.md` first. If it is missing, load the `ns-setup` skill instead of guessing. Then answer each question from the codebase, and ask the user only what you cannot observe:

- **Surface**: what does a user touch? A CLI or TUI, an HTTP service, a library, a board over serial, a device on a bench. Pick the primary surface and list the rest.
- **Run**: how does it start locally? Prefer the repo's own documented command (Makefile, `pyproject.toml` scripts, README). Note ports, env vars, seed data, auth. Firmware: the build, flash and reset commands, and the probe, from `stack.md`.
- **Drive**: what can an agent script? Existing harnesses first (pytest e2e suites, expect scripts, HIL runners, a debug port). Only then a generic recipe from [drive-recipes.md](references/drive-recipes.md).
- **Observe**: what evidence exists? Terminal transcripts, response bodies, logs, exit codes, stored state, serial logs, logic-analyzer or power captures.
- **Isolate**: can two instances run side by side (ports, data dirs)? A bench with one board cannot. Say so in the generated skill: refusing to double-drive a shared instance beats corrupting it.

If the checkout doesn't build or start as-is, fix that first or report it precisely. A skill written against a broken base teaches wrong steps.

Done when every question has an answer, from code or from the user, and you know which questions only hardware or credentials can answer.

## 2. Pick the locations

- **Skill**: the repo's agent skills dir if one exists (`.agents/skills/`, `.claude/skills/`, `.cursor/skills/`, checked in that order), else `.agents/skills/`. The user may name another. The skill lives at `<dir>/verify-<project>/`.
- **Control CLI**: the repo's language and conventions. Default is Python with `argparse`, or `click` if the repo already uses it. Put it in the repo's existing `scripts/` or `tools/` dir, else `tools/control-<project>.py`. If the repo has a Python package with console scripts, add a `control-<project>` entry point too.
- **Evidence**: `.ns/verify/<run-id>/` by default, overridable with `--out`. Add `.ns/` to `.git/info/exclude` if it is not there. `ns-verify` passes `--out .ns/<unit-id>/evidence/`.

## 3. Build the control CLI

Copy [control-cli-template.py](references/control-cli-template.py) and fill it in. It already has `doctor`, `launch`, `exec` and `cleanup`, JSON output, `--dry-run`, a state dir, and pidfile-based cleanup. Add the project's verbs from [drive-recipes.md](references/drive-recipes.md): `http get`, `tui send/expect`, or for firmware `build`, `flash`, `serial send/expect`, `reset`, `capture`, `hil run <case>`.

Every command follows [cli-for-agents.md](references/cli-for-agents.md). Read it in full before writing the first command.

Done when no `FILL:` marker remains, `--help` on every subcommand shows working examples, and each rule in the reference's review checklist holds.

## 4. Generate the skill

Write `<dir>/verify-<project>/SKILL.md`. Frontmatter carries `name: verify-<project>` and a `description` naming the project, the surface, and when to reach for it. Without frontmatter the skill never registers. Each section below holds real commands from this repo, with no placeholders:

- **Launch**: the exact command that starts the thing for verification (`control-<project> launch`), how to tell it's ready (a log line, a port answering, a prompt, a boot banner on serial), and teardown. A short-lived CLI has no server: launch means build once, then start each drive in its own PTY. Firmware: build, flash, reset, wait for the banner.
- **Doctor**: one read-only check that answers "is this instance worth driving?" (`control-<project> doctor`). Process up, right build, port owned by us, auth valid. Firmware: probe attached, board responds, firmware version matches the build. An agent runs this first whenever anything looks off.
- **Drive**: the harness recipe with real commands, prompt strings, routes and serial commands from this repo. Prefer stable handles (command names, route paths, prompt text, register names) over timing and screen positions.
- **Evidence**: what to capture and where it goes. State the proof standards:
  - Exercise the real user path, not internal setters or test-only endpoints.
  - Capture the action and the resulting state, not just the final output.
  - Verify side effects (files written, rows stored, pins toggled, messages sent) alongside what is printed.
  - Mock only where a production boundary already isolates the external system.
  - A dry-run or test mode may still touch the network or hardware. Verify what it skips by observing, not by trusting its name.
- **Cleanup**: `control-<project> cleanup`. Kill what you started, never by process name. Cleanup removes instances and scratch state and leaves the evidence in the named location. Firmware: leave the board in a known state (reset, or reflash the baseline) and release the probe and serial port.
- **Helpers**: every script the skill ships is executable and its invocation is shown in the skill body.

## 5. Seed the feature map

Create `<dir>/verify-<project>/features/README.md` plus one file per user-facing feature. Start with the top 3 to 5, found from commands, routes, shell commands or docs. Follow [feature-map-example/](references/feature-map-example/): a README with baseline preconditions, driving conventions, proof rules and an index, then one file per feature with exactly these four H2s in order:

1. `Sub-features`
2. `How to get to it (user POV)`
3. `Driving it with control-<project>`
4. `Gotchas`

The map is the repo's maintained verification source. A proof that drives one convenient entry point is incomplete when the map lists others.

## 6. Point docs/agents/verify.md at both

`ns-setup` writes `docs/agents/verify.md` with a "Not set up yet" variant. Replace that variant with this one, or create the file in this shape if it is missing:

```markdown
# Verify

How to prove a change works on the real surface, beyond the test suite. `ns-verify` reads this file.

## Set up

- **Verify skill**: `verify-<project>` at `<dir>/verify-<project>/SKILL.md`, feature map in `features/`. Load it to drive the product and collect evidence.
- **Control CLI**: `<invocation>` (`<path>`). Start with `<invocation> doctor`, then `<invocation> --help`.
- **Evidence**: `.ns/verify/<run-id>/` by default. Pass `--out <dir>` to override.
- **Surfaces**: covered: <primary>. Not covered: <list, and why>.
- **Last proven end to end**: <date>, <commit sha>, feature `<id>`.
```

## 7. Prove the generated skill before handing it over

Run the generated skill's own instructions once, end to end, from a fresh shell:

1. Launch.
2. Doctor, and require it to pass.
3. Drive one mapped feature. One is enough; the map exists so later runs cover the rest.
4. Capture evidence to the named location.
5. Clean up.
6. Confirm the evidence still exists. A cleanup that eats the proof fails this step.

Fix what fails, and run the generated cleanup after every failed attempt too, so broken attempts don't strand processes, ports or a held probe. A generated skill that was never executed is a draft, not a deliverable.

If the proof needs something you can't get (no board attached, no credentials), ship it as a draft. Write `Last proven end to end: never (blocked: <what is missing>)` in `verify.md` and report the blocker.

Done when steps 1 to 6 ran clean in one pass and `verify.md` records the date, sha and feature.

## 8. Hand over

Report the skill path, the CLI path, the proven feature and its evidence path, and the surfaces left uncovered. Recommend committing both so every agent drives the project the same way.

Feature maps rot as the project changes. `ns-maintain-verify` (planned) is the upkeep loop: it re-checks each feature file against source, drives every feature live, and opens one PR of corrections or reports a product regression. Suggest a cadence only if asked.
