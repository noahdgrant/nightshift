---
name: ns-agent-readiness
description: Audit a codebase for what helps or hurts agents working in it (competing ways to do one job, rules held only by comments, unusual dependencies, oversized files, shallow modules, no verify loop), write a ranked report, and file the top findings. Use when onboarding a repo, or when asked how agent-friendly a codebase is or where agents keep copying the wrong pattern.
metadata:
  upstream:
    - mattpocock/skills@c55ee46073ed:skills/engineering/improve-codebase-architecture
    - mattpocock/skills@c55ee46073ed:skills/engineering/codebase-design
---

# Agent readiness

Agents take the shortest path and copy the code around them. Find the places where that path leads them wrong, rank them, and turn the top ones into issues. The checks follow [pave the road](../ns-principle-pave-the-road/SKILL.md), plus navigability and the verify loop.

Talk about the architecture in the deep-module vocabulary from [checks.md](references/checks.md#vocabulary) (**module**, **interface**, **depth**, **seam**, **adapter**, **leverage**, **locality**), and about the domain in the words of the project's glossary (`docs/agents/domain.md` says where it is). ADRs record decisions; raise a finding that contradicts one only when the friction is real enough to reopen it, and say so in the finding.

The run changes no code. Its outputs are the report and the issues it files.

## 1. Scope

Put the weight on what changes. A finding in a file nobody touches costs agents nothing.

- If the user named a direction (a module, a subsystem, a pain point), take it and skip the rest of this step.
- Otherwise find the **hot spots**: the files and directories that keep coming up in recent history.

  ```bash
  git log --since="6 months ago" --name-only --format= | grep . | sort | uniq -c | sort -rn | head -30
  ```

  Widen the window when it returns little. When the repo has fewer than about 20 commits, or the changes are scattered with no clear hot spot, the whole tracked tree is in scope.

Then read `docs/agents/verify.md`, `docs/agents/issue-tracker.md`, `docs/agents/triage-labels.md`, the glossary, and any ADRs in the hot spots.

Done when you have a written list of hot spots, each with its commit count, or a note that the whole tree is in scope and why.

## 2. Scan

Hand the hot spots and [checks.md](references/checks.md) to a **fresh-context agent**. It walks the code organically and notes friction, with the five check groups as its lens. Without one, scan inline and say so in the report.

A finding needs evidence: the files and lines, and the second copy, the comment or the missing check that makes it a finding. A hunch with no file is not a finding.

Done when every check group has been applied to every hot spot, and each group has its findings or a line saying it found none.

## 3. Rank and write the report

Write the report to `.ns/agent-readiness/<UTC timestamp, YYYYMMDD-HHMM>/report.md`. `.ns/` stays out of git, so nothing lands in the repo. Create the directory if it is missing.

Give each finding a **strength**:

- **Strong**: agents will copy or trip on this, and the evidence shows it (two live copies, a rule already broken once, a hot spot with no test).
- **Worth exploring**: real friction, but the fix has a cost or a judgement call worth a human look.
- **Speculative**: it might bite. Keep it in the report and out of the issue list.

Rank Strong first, then Worth exploring, then Speculative. Within a strength, rank by how often an agent meets the problem: hot spot commit counts, number of call sites.

```markdown
# Agent readiness: <repo>, <date>

Scope: <hot spots and their commit counts, or "whole tree" and why>
Scan: <fresh-context agent, or inline>

## Findings

### 1. <title>
- **Strength:** Strong | Worth exploring | Speculative
- **Group:** <check group>
- **Files:** <path:line, one per file involved>
- **What an agent gets wrong:** <what it would copy, skip or break, and when>
- **Fix:** <the change, at the highest rung of the ladder that works, and the rung's name>

### 2. ...

## Groups with no findings
<group>: <what was checked>

## Filed
<issue reference and finding number, one per line, or what the human picked, or "none">
```

The fix sits as high up this ladder as works: architecture that makes the wrong way unwritable, then lint, compiler and CI, then skills and rules, then review. [Encode lessons in structure](../ns-principle-encode-lessons-in-structure/SKILL.md) has the mechanisms. Two roads for one job are fixed by [migrating the callers to one and deleting the other](../ns-principle-migrate-callers-then-delete-legacy-apis/SKILL.md); name which road to keep and why.

Done when every finding has all five fields and a rank, and the report is written.

## 4. File

Read the cap from the `audit issues per run: <n>` line in `docs/agents/issue-tracker.md`. With no such line, the cap is 5.

- **`gates: auto`**: take the Strong and Worth exploring findings in rank order. For each, search the tracker for a duplicate as `issue-tracker.md` says. Skip a duplicate and note it under Filed; it does not count toward the cap. File the rest until the cap is reached.
- **`gates: stop`** (the default): show the ranked list with each finding's strength and the cap, ask which to file, and file only the ones the human picks.

File each issue as `issue-tracker.md` says, in the `needs-triage` state with a category from `triage-labels.md` (`bug` when the finding hides a defect, else `enhancement`) and every area label that file defines for the files involved. Title it with the problem in plain words. The body holds the finding's fields, the report's date, and the line `Found by ns-agent-readiness.` Triage sets priority and writes the brief.

Done when the issues are filed, at most the cap of them, and the report's Filed section lists each one.

## 5. Report back

Give the report's path, the top three findings by title and strength, and the issues filed.
