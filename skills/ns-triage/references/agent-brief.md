# Writing Agent Briefs

An Agent Brief is a structured comment posted on an issue or PR when it moves to `ready-for-agent`. The same text goes into the unit's `.ns/<unit-id>/brief.md`, which `ns-build` reads. It is the authoritative specification an AFK agent works from. The original body and discussion are context: the Agent Brief is the contract.

The brief states **what the agent should do**, which stretches to both surfaces: for an issue, that's building the change from nothing; for a PR, it's what's left to do *to the existing diff*: finish it, close gaps, address review points. Same principles either way; the PR example below shows the difference.

## Principles

### Durability over precision

The issue may sit in `ready-for-agent` for days or weeks. The codebase will change in the meantime. Write the brief so it stays useful even as files are renamed, moved, or refactored.

- **Do** describe interfaces, types, and behavioral contracts
- **Do** name specific classes, function signatures, or config shapes that the agent should look for or modify
- **Do** name symbols, never file paths or line numbers: paths and lines go stale
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

### Explicit scope boundaries

State what is out of scope. This keeps the agent from gold-plating or making assumptions about adjacent features.

### Route when the next phase isn't obvious

`**Route:**` is optional. A brief that moves to `ready-for-agent` goes to `ns-build`, so triage usually leaves it out. `ns-troubleshoot` always writes it, because its brief can go to `ns-build`, to `ns-define` for a redesign, or nowhere. `ns run` reads only the frontmatter `status`, so the Route never overrides it.

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
Be specific about edge cases and error conditions.

**Decisions:** each settled design decision and who settled it, quoted (omit when none; see [design decisions](../SKILL.md#design-decisions))

**Key interfaces:**
- `ClassName`: what needs to change and why
- `function_name()` return type: what it currently returns vs what it should return
- Config shape: any new configuration options needed

**Size:** files and modules touched, new tests, rough changed lines

**Acceptance criteria:**
- [ ] Specific, testable criterion 1
- [ ] Specific, testable criterion 2
- [ ] Specific, testable criterion 3

**Out of scope:**
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
and append "..." to indicate truncation.

**Key interfaces:**
- The `SkillMetadata` dataclass's `description: str` field: no type change
  needed, but the logic that populates it needs to respect word boundaries
- `parse_frontmatter(text: str) -> SkillMetadata`, or whatever function reads
  SKILL.md frontmatter and extracts the description

**Size:** one module (frontmatter parsing), four pytest cases, about 40 lines

**Acceptance criteria:**
- [ ] Descriptions under 1024 chars are unchanged
- [ ] Descriptions over 1024 chars are truncated at the last word boundary
      before 1024 chars
- [ ] Truncated descriptions end with "..."
- [ ] The total length including "..." does not exceed 1024 chars
- [ ] A pytest case covers each of the above

**Out of scope:**
- Changing the 1024 char limit itself
- Multi-line description support
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

**Key interfaces:**
- Markdown file format in `.out-of-scope/`: each file should have a
  `# Concept Name` heading, a `**Decision:**` line, a `**Reason:**` line,
  and a `**Prior requests:**` list with issue links
- The triage workflow should read all `.out-of-scope/*.md` files early
  and match incoming issues against them by concept similarity

**Size:** the triage workflow and a new `.out-of-scope/` format, no code tests, about 120 lines

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

**Key interfaces:**
- The command's error path should emit `{"error": "<message>"}` under `--json`
  instead of the plain-text error
- Reuse the `to_json()` serializer the PR already added; keep it the only one

**Size:** the command's error path, two pytest cases, about 50 lines on top of the PR

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
- References file paths and line numbers that will go stale
- No acceptance criteria
- No scope boundaries
- No description of current vs desired behavior
