---
name: ns-requirements
description: Write, review, or change a requirements doc, MoSCoW-prioritized, with user stories where a person uses the thing. Use when the user wants to capture, prioritize, or critique requirements, flesh out requirements for a new project, mentions MoSCoW, must-haves, or user stories, or before a design doc when no requirements exist.
---

# Requirements

A requirements doc says what must be true when a piece of work is done, and what gets cut first when time runs out. Every line is something a tester could pass or fail. The design doc says how; this doc never does.

## Where docs live

Read `docs/agents/docs.md` first. It names the requirements directory, the review surface, the ID keys already in use, and any template override. If it is missing, load the `ns-setup` skill instead of guessing.

The template is `docs/agents/templates/requirements.md` in the target repo when that file exists, else [references/template.md](references/template.md).

## Where a doc sits

Every doc has one **kind**, stated in its header.

| Kind | When | Holds | Rows trace to |
|---|---|---|---|
| Standalone | The thing answers to no larger system: a tool such as a CLI, a new project, a one-off | User stories and every requirement | A story, a stated business, legal, or customer need, or another doc's requirement it must obey, cited by ID |
| System | A product built from several components with their own docs | User stories, and every promise made to a customer, the business, or a regulator, each with an *Allocated to* column naming the components that deliver it | A story or a stated need |
| Component | One part of a system | What that component must do to meet its system's requirements, nothing the system doc already says | A system requirement ID, a system story it meets directly, or a sourced constraint |

Most docs are standalone. Reach for system and component docs when several components each need their own requirements and must agree on shared promises. Each doc has one owner, so where ownership splits between teams, the docs split too.

Each requirement lives in one doc only. A promise stays in the system doc even when one component delivers it alone; the component doc refines it ("sends a payment to the server within 5 s" under "the driver sees the paid-until time within 10 s") and cites its ID. A component row never repeats its parent, and never outranks it: a component Must that serves only a system Should means one of the two priorities is wrong. A system row allocated to a component that needs no refinement there is named on the component doc's "Met as written" line, by ID only, so a reader sees every promise the component carries without a copy that can drift.

## Scope and MoSCoW

The scope is whatever the priorities are relative to: a release ("meter V1"), a feature, a pilot, a customer contract, a first usable version. It goes in the header, and MoSCoW means nothing without it:

- **Must**: the scope is not done without it.
- **Should**: important, but the scope can be declared done without it if time runs out.
- **Could**: included if there is time.
- **Won't**: not in this scope, with the reason. Not the same as never.

The test for a Must: "If everything else were finished, would we still refuse to call <scope> done?"

## Versions and the changelog

Each thing gets its own directory under the requirements directory from `docs/agents/docs.md`, named for the thing in lowercase (`system`, `meter`, `cli`) rather than its ID key. It holds one file per version (`v1.md`, `v1.1.md`, `v2.md`) and one `changelog.md` covering all of them.

Start a new version for a new scope, a rewrite, or any change to an approved version. A draft changes in place; an approved version never does, because designs and child docs were checked against it. Each change to a version adds one line to the changelog, newest first, so a reader sees what each review round changed. The first line is the one that created the version. Earlier versions stay as they were, so a design doc that cites one still reads what its author read.

## User stories

Standalone and system docs start from user stories when a person is on the other end: "As a <actor>, I want <capability>, so that <benefit>." The actor is a person in a role (driver, meter technician, city enforcement officer, an engineer running the CLI), never a system. Stories record the need; requirements record the commitment. Each requirement's "Why" cites the stories it serves, and a story with no requirement is either a Won't or a gap.

## Workflow

1. **Interview.** Load the `ns-grilling` skill and run it over the design tree in [references/interview.md](references/interview.md). When no one can answer (an unattended run, `gates: auto`), skip the interview: draft from the material at hand, and turn each branch it leaves unsettled into `[?]` with an Open questions row. See [never block on the human](../ns-principle-never-block-on-the-human/SKILL.md).
2. **Draft** from the template.
3. **Self-review.** Run [references/rubric.md](references/rubric.md) against the draft and fix what fails before showing the author.
4. **Save and hand off.** Write the version file and its changelog entry to the thing's directory, with every `>` prompt line removed. Set Status to `Draft`, or `In review` once it goes to reviewers. Tell the author:
   - The repo file is the source of truth, changed through pull requests.
   - How it reaches reviewers, per the review surface in `docs/agents/docs.md`. Review comments come back as edits to the repo file.
   - Approval is theirs to give.
   - When the next step is a design, the `ns-design-doc` skill picks up from here.

## Approval

Approving requirements is human-only ([factory contract](../ns-contract/SKILL.md#gates)). Leave Status at `Draft` or `In review`; a person sets `Approved` and `Superseded by v<n>`.

## Hard rules

- Write in the terms of the repo's `GLOSSARY.md` when one exists.
- Each ID is `<KEY>-<n>`, where the key names the thing (`METER-5`, `CLI-3`). Take the key from `docs/agents/docs.md` when the thing has one there; a new thing gets a new key, added to its table. IDs are never renumbered or reused, because design docs, tickets, and child docs cite them.
- One requirement per row. An "and" joining two checks is two rows.
- A requirement names the outcome, not the mechanism. "Show the driver the paid-until time within 10 s of payment" is a requirement; "push it over MQTT" is a design choice. Name a mechanism only when it is itself required, such as a city contract that mandates contactless card payment, and cite that source.
- Give the source of every number: measured, estimated, datasheet, customer, or regulation. Outside facts go in the Sources section with a link to the primary source, and rows cite them by number. A number with no source gets treated as fact by the next reader.
- Cite other docs by name and version ("PARK v1"), never as "the old doc".
- When a number comes from another of the project's docs that is its source, such as a design's measured budget or an interface's limit, the row's Why / source cites that doc's ID or section instead of repeating its figures, so the row stays right when the source changes.
- A value nobody has decided yet is written `[?]` and gets a row in Open questions with an owner by name and a Needed by milestone, such as the beta or an issue. A milestone stays true when its date moves; use a calendar date only when no milestone fits. Never invent a number to fill the gap.

## Changing a requirement

Every change is a check on everything built against the doc.

1. If the current version is approved, start the next one (`v1.1.md` to `v1.2.md`) as a copy with Status `Draft`. The approved one becomes `Superseded by v<n>` when a person approves the new one. Then edit the draft's repo file, the source of truth, in a pull request. Add its line to the changelog, as "Versions and the changelog" says.
2. A dropped requirement moves to Won't with its ID and the reason. It keeps its ID so older citations still resolve.
3. In a system doc, check each child doc's rows that cite the changed ID, and fix any priority or refinement the change broke.
4. Update `Last updated` in the header. Search the design docs directory from `docs/agents/docs.md` for the changed IDs and for the doc's name. For each approved design doc that cites one, follow "When requirements change" in the `ns-design-doc` skill. In a draft design doc, fix any goal or decision the change broke, and point its Requirements row at the new version if the change started one.

## Reviewing an existing doc

Score it against [references/rubric.md](references/rubric.md). Lead with the two or three problems that would cost the most during design or acceptance. Then rewrite a few representative rows in the template's table format, keeping `[?]` wherever the fact is unknown. When the doc has no IDs, propose them in its current order. When it belongs to a system, read the other docs in the tree too: the costliest problems are usually between docs, such as a promise no component owns or a priority that disagrees with its parent.

When updating a doc that is already circulating, keep its structure and fix the rows. Moving it to the new template is a separate change, and it's the author's call.
