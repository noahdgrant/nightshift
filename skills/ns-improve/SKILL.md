---
name: ns-improve
description: Read the grades that did not pass and propose units that fix their cause, only for a cause that at least three different units failed on the same rubric. Use when asked to improve the factory from its grades.
---

# Improve

You turn failed grades into units that fix their cause. You are given the grades that did not pass (unit, rubric, score, reason), the open improvement units, the repository to read and sometimes the `Fix level:` lines of recent review findings. Read the code and docs and change nothing.

## 1. Group the grades by cause

Group the grades of one rubric by the cause their reasons share. Read the code or docs the reasons point to, so the cause is something you checked, not a guess from the wording.

Done when each group names its rubric, its cause and the different units it holds.

## 2. Keep the causes that repeat

Propose a unit only for a cause that at least 3 different units failed on the same rubric. One unit failing twice counts once. A cause that an open improvement unit already covers gets no new proposal. When no cause repeats, say so and propose nothing.

Done when every cause you keep has 3 or more different units.

## 3. Name the layer and write the brief

Choose the layer to fix:

- `code`: a type, boundary or interface shape that makes the mistake impossible.
- `check`: a lint rule, test or CI check that catches the category.
- `prose`: a doc or coding standard, for what only judgement can enforce.

Choose the highest layer that removes the whole category, `code` then `check`, and `prose` only when neither fits.

Write the brief in the repository's unit format: Goal, Requirements, Out of scope, Open questions. Cite the units and grades behind the cause. Propose changes only to what the repository owns: `docs/agents/*`, `AGENTS.md`, the coding standards in force, `.nightshift/check`, lint rules, tests and code. The text of a skill lives in the `ns` binary and the repository cannot patch it, unless the repository has its own `skills/ns-*` directories and the prompt says so. When the cause lies in a skill the repository cannot patch, put it under a heading `For the skill's owner` and propose no change to the skill.

Done when each brief cites its units and names one layer.

## 4. End with the proposals

When you propose nothing, end with the line `NOTHING TO PROPOSE`. Otherwise end with one block for each proposal, and nothing after them:

```
PROPOSAL
TITLE: <type>(<scope>): <subject>
RUBRIC: <the rubric's name>
CAUSE: <the cause as a short lowercase slug of letters, digits and hyphens>
LAYER: <code, check or prose>
LABELS: <a type: label, a priority: label and an area: label the repository has, comma separated>
BRIEF:
<the brief, in Markdown, to the next PROPOSAL line or the end>
```

The title is a Conventional Commit. Do not write a disclosure or the cause marker: `ns` adds them.
