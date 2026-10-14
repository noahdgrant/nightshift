# Writing Agent Briefs

An Agent Brief is a structured comment posted on an issue or PR when it moves to `ready-for-agent`. The same text goes into the unit's `.ns/<unit-id>/brief.md`, which `ns-build` reads. It is the authoritative specification an AFK agent works from. The original body and discussion are context: the Agent Brief is the contract.

The brief states **what the agent should do**, which stretches to both surfaces: for an issue, that's building the change from nothing; for a PR, it's what's left to do *to the existing diff*: finish it, close gaps, address review points. Same principles either way; the PR example below shows the difference.

## Principles

### Durability over precision

The issue may sit in `ready-for-agent` for days or weeks. The codebase will change in the meantime. Write the brief so it stays useful even as files are renamed, moved, or refactored.

- **Do** describe interfaces, types, and behavioral contracts
- **Do** name specific classes, function signatures, or config shapes that the agent should look for or modify
- **Do** name symbols, never file paths or line numbers: paths and lines go stale. The [Touches](#touches-the-one-place-for-paths) line is the one exception
- **Do** expect the current implementation structure to change

Firmware note: name the Kconfig symbol, devicetree node, register or HAL call (`CONFIG_UART_RX_TIMEOUT_MS`, the `uart0` node), not the source file that happens to touch it.

### Behavioral, not procedural

Describe **what** the system should do, not **how** to implement it. The agent will explore the codebase fresh and make its own implementation decisions.

- **Good:** "The `SkillConfig` dataclass should accept an optional `schedule: CronExpression | None` field"
- **Bad:** "Open src/skills/config.py and add a schedule field on line 42"
- **Good:** "When a user runs `triage` with no arguments, they should see a summary of issues needing attention"
- **Bad:** "Add an if/elif chain in the main handler function"

### Complete acceptance criteria

The agent needs to know when it's done. Every Agent Brief has concrete, testable acceptance criteria. Each criterion is independently verifiable, ideally by a test or a command in `docs/agents/stack.md`.

- **Good:** "Running `gh issue list --label needs-triage` returns issues that have been through initial classification"
- **Bad:** "Triage should work correctly"

### Ship note: PR text

A criterion is verified before the PR exists, so anything only the PR can show stays out of **Acceptance criteria**: its `Closes #N` line, the issues its body names, its labels, its merge. Put each such item under **Ship note** instead. `ns-ship` checks the note against the PR body before it opens the PR. Omit the section when there is nothing to say; `ns-ship` writes `Closes #N` from the issue number either way.

- **Good:** Ship note: "The PR body names #88 as the follow-up for the listing summary."
- **Bad:** Acceptance criterion: "The PR body says `Closes #563` and names #536."

### Failure modes, edge cases and scope, from the code

Every brief carries **Failure modes**, **Edge cases** and **Out of scope**. Where the brief is silent, build guesses, and review flags the guess. A section that does not apply says so in one line (`Edge cases: none; the command takes no input.`), so a reader can tell "considered" from "forgotten".

Fill them from the code paths the change involves, found while exploring the codebase, not from a generic list:

- **Failure modes**: what happens on each error the path can raise and each dependency that can be unavailable or invalid (a missing or corrupt file, a failed call, bad input), and what the user sees: message, exit code, state left behind.
- **Edge cases**: the empty, unset, boundary, concurrent and resumed states that apply to this path, and the behaviour each one gets.
- **Out of scope**: what the agent must leave alone. This keeps it from gold-plating or making assumptions about adjacent features.

### Instances: fix the class, not the report

A brief that fixes a defect carries **Instances**: the search run for every other place the same defect can occur, and each hit. That holds for a `bug`, and for an enhancement or `ready-for-human` brief that fixes a defect. Search for the defect's shape, not the reporter's wording: a `grep` or `ast-grep` pattern for the faulty construct, or the callers and copies you read. A search that finds only the reported place says so, with the search, so a reader can tell "searched" from "forgot".

Each hit is covered by this brief, or names the issue it was filed under (`filed separately: #536`). Build fixes every covered hit and adds a guard for the class, and review flags a fix that leaves one out. When the hits are too many for one unit, split them per [triage](../SKILL.md) step 5, each child with its own Instances.

### Route when the next phase isn't obvious

`**Route:**` is optional. A brief that moves to `ready-for-agent` goes to `ns-build`, so triage usually leaves it out. `ns-troubleshoot` always writes it, because its brief can go to `ns-build`, to `ns-define` for a redesign, or nowhere. `ns run` reads only the frontmatter `status`, so the Route never overrides it.

### Touches: the one place for paths

Every `ready-for-agent` brief carries a `**Touches:**` line: the path globs, relative to the repo root, for the files and directories the unit will probably change. It is a scheduling prediction, not part of the spec, so it is the one place a brief names paths. Name a file the unit edits (`cli/src/run.rs`), or a directory glob where it adds or edits several files (`skills/ns-review/**`). Include the tests and docs it will change. Paths only, never line numbers.

Write the line as backticked globs separated by commas, all on one line: `` **Touches:** `cli/src/run.rs`, `skills/ns-review/**` ``. `brief.md` repeats the list in its frontmatter as `touches:`, one glob per item, quoted when it starts with `*` (`- "**/Cargo.toml"`). A line in any other shape reads as no prediction.

### Sized to one unit

Record the size estimate in the brief; the soft limit and the split are in [triage](../SKILL.md) step 5.

## Template

```markdown
## Agent Brief

**Category:** bug / enhancement
**Route:** ns-build / ns-define / none (optional: the phase that picks the brief up)
**Summary:** one-line description of what needs to happen

**Current behavior:**
Describe what happens now. For bugs, this is the broken behavior.
For enhancements, this is the status quo the feature builds on.

**Desired behavior:**
Describe what should happen after the agent's work is complete.

**Failure modes:** (required)
- Each error or unavailable dependency on the code path: what the user sees
  (message, exit code) and what state is left
- Or one line: "none; <why the path cannot fail>"

**Edge cases:** (required)
- Each empty, unset, boundary, concurrent or resumed state that applies, and
  its behaviour
- Or one line: "none; <why none apply>"

**Instances:** (required when the brief fixes a defect)
- Search: the grep, ast-grep pattern or reading that looked for the defect's
  shape everywhere
- `symbol_name()`: the reported instance, covered
- `other_symbol()`: covered, or "filed separately: #N"
- Or one line: "only the reported instance; <search> found no other"

**Decisions:** each settled design decision and who settled it, quoted (omit when none; see [design decisions](../SKILL.md#design-decisions))

**Key interfaces:**
- `ClassName`: what needs to change and why
- `function_name()` return type: what it currently returns vs what it should return
- Config shape: any new configuration options needed

**Size:** files and modules touched, new tests, rough changed lines

**Touches:** `path/to/file.rs`, `dir/**`

**Acceptance criteria:**
- [ ] Specific, testable criterion 1
- [ ] Specific, testable criterion 2
- [ ] Specific, testable criterion 3

**Ship note:** (optional: what the PR itself must carry, never an acceptance criterion)
- PR text: issues the body names, labels it needs

**Out of scope:** (required)
- Thing that should NOT be changed or addressed in this issue
- Adjacent feature that might seem related but is separate
```

## Examples

### Good Agent Brief (bug)

```markdown
## Agent Brief

**Category:** bug
**Summary:** Skill description truncation drops mid-word, producing broken output

**Current behavior:**
When a skill description exceeds 1024 characters, it is truncated at exactly
1024 characters regardless of word boundaries. This produces descriptions
that end mid-word (e.g. "Use when the user wants to confi").

**Desired behavior:**
Truncation should break at the last word boundary before 1024 characters
and append "..." to indicate truncation. The same rule applies to every field
listed under Instances, through one shared helper.

**Instances:**
- Search: `grep -rn '\[:1024\]\|\[:MAX_' src/` for fixed-width slices, then
  read each hit's caller
- `parse_frontmatter()`, the description field: the reported instance, covered
- `parse_frontmatter()`, the `argument-hint` field: the same 1024-char slice,
  covered
- `render_listing()`, the one-line summary: cuts at 80 chars mid-word, filed
  separately: #88

**Failure modes:** none; truncation is pure string handling on a description
the parser has already read, and a parse error keeps its current message.

**Edge cases:**
- A description of exactly 1024 chars is unchanged
- A first word longer than 1021 chars has no boundary to break at: cut it at
  1021 chars and append "..."
- An empty or missing description stays empty, with no "..."

**Key interfaces:**
- The `SkillMetadata` dataclass's `description: str` field: no type change
  needed, but the logic that populates it needs to respect word boundaries
- `parse_frontmatter(text: str) -> SkillMetadata`, or whatever function reads
  SKILL.md frontmatter and extracts the description

**Size:** one module (frontmatter parsing), five pytest cases, about 50 lines

**Touches:** `src/skills/frontmatter.py`, `tests/test_frontmatter.py`

**Acceptance criteria:**
- [ ] Descriptions under 1024 chars are unchanged
- [ ] Descriptions over 1024 chars are truncated at the last word boundary
      before 1024 chars
- [ ] Truncated descriptions end with "..."
- [ ] The total length including "..." does not exceed 1024 chars
- [ ] A pytest case covers each of the above
- [ ] The `argument-hint` field truncates by the same rule

**Ship note:**
- The PR body names #88 as the follow-up for the listing summary

**Out of scope:**
- Changing the 1024 char limit itself
- Multi-line description support
- The listing summary (#88)
```

### Good Agent Brief (enhancement)

```markdown
## Agent Brief

**Category:** enhancement
**Summary:** Add `.out-of-scope/` directory support for tracking rejected feature requests

**Current behavior:**
When a feature request is rejected, the issue is closed with a `wontfix` label
and a comment. There is no persistent record of the decision or reasoning.
Future similar requests require the maintainer to recall or search for the
prior discussion.

**Desired behavior:**
Rejected feature requests should be documented in `.out-of-scope/<concept>.md`
files that capture the decision, reasoning, and links to all issues that
requested the feature. When triaging new issues, these files should be
checked for matches.

**Failure modes:**
- No `.out-of-scope/` directory: triage treats it as no prior rejections and
  creates the directory on the first wontfix
- A file without a `**Decision:**` line: triage names the file in its notes
  and skips it for matching, never fails the run

**Edge cases:**
- Two rejections of one concept on the same day append to one file, no
  duplicate entry for the same issue link
- A reopened issue already listed in "Prior requests" stays listed once

**Key interfaces:**
- Markdown file format in `.out-of-scope/`: each file should have a
  `# Concept Name` heading, a `**Decision:**` line, a `**Reason:**` line,
  and a `**Prior requests:**` list with issue links
- The triage workflow should read all `.out-of-scope/*.md` files early
  and match incoming issues against them by concept similarity

**Size:** the triage workflow and a new `.out-of-scope/` format, no code tests, about 120 lines

**Touches:** `skills/triage/**`, `.out-of-scope/**`

**Acceptance criteria:**
- [ ] Closing a feature as wontfix creates/updates a file in `.out-of-scope/`
- [ ] The file includes the decision, reasoning, and link to the closed issue
- [ ] If a matching `.out-of-scope/` file already exists, the new issue is
      appended to its "Prior requests" list rather than creating a duplicate
- [ ] During triage, existing `.out-of-scope/` files are checked and surfaced
      when a new issue matches a prior rejection

**Out of scope:**
- Automated matching (human confirms the match)
- Reopening previously rejected features
- Bug reports (only enhancement rejections go to `.out-of-scope/`)
```

### Good Agent Brief (PR)

For a PR, "Current behavior" describes the state of the diff, and the brief asks the agent to finish or fix it rather than build from scratch.

```markdown
## Agent Brief

**Category:** enhancement
**Summary:** Finish the contributor's `--json` output flag for `triage list`

**Current behavior:**
The PR adds a `--json` flag that serializes the issue list to JSON. The happy
path works and the diff matches the project's command structure. Two gaps
remain: errors are still printed as human text (not JSON), and the new flag has
no test coverage.

**Desired behavior:**
With `--json`, all output (including errors) is well-formed JSON on stdout,
and the command's exit codes are unchanged. The existing human-readable output
is untouched when the flag is absent.

**Failure modes:**
- The tracker is unreachable: under `--json`, `{"error": "<message>"}` on
  stdout and the same non-zero exit code as without the flag
- An argument error from the parser: same shape, same exit code

**Edge cases:**
- No matching issues: `[]`, exit 0
- A title holding quotes or newlines comes out escaped, still valid JSON

**Key interfaces:**
- The command's error path should emit `{"error": "<message>"}` under `--json`
  instead of the plain-text error
- Reuse the `to_json()` serializer the PR already added; keep it the only one

**Size:** the command's error path, two pytest cases, about 50 lines on top of the PR

**Touches:** `src/triage/cli.py`, `tests/test_cli.py`

**Acceptance criteria:**
- [ ] `triage list --json` emits valid JSON for both success and error cases
- [ ] Exit codes match the non-JSON command
- [ ] A pytest case covers the `--json` success output and one error case
- [ ] Default (non-JSON) output is byte-for-byte unchanged

**Out of scope:**
- Adding `--json` to any other command
- Changing the JSON shape of the success payload the PR already defined
```

### Bad Agent Brief

```markdown
## Agent Brief

**Summary:** Fix the triage bug

**What to do:**
The triage thing is broken. Look at the main file and fix it.
The function around line 150 has the issue.

**Files to change:**
- src/triage/handler.py (line 150)
- src/triage/models.py (line 42)
```

This is bad because:
- No category
- Vague description ("the triage thing is broken")
- References file paths and line numbers in the spec, where they go stale, instead of path globs on a `**Touches:**` line
- No acceptance criteria
- No failure modes or edge cases, so build guesses them
- No Instances, so the fix covers one place and the next ticket reports the copy
- No scope boundaries
- No description of current vs desired behavior
