# Design doc template

Lines beginning with `>` are prompts. Delete every one before the doc goes out.

## Which sections to include

Always-sections stay in every doc. If one genuinely has nothing to say, write one line saying so, because an empty-but-present section tells a reviewer you looked. Triggered sections are deleted when their trigger does not fire.

| Section | Include |
|---|---|
| Summary | Always |
| Problem | Always |
| Requirements | Always |
| Values, under Requirements | When another doc states a value this design sets, such as a time, a size, or a limit |
| Constraints | Always |
| Architecture | When the change adds a component or changes how existing ones talk |
| Design | Always |
| Alternatives considered | Always |
| Failure modes and risks | Always |
| Privacy | Always. One line if the change touches no personal data, the full section if it carries, stores, or exposes identity, location, contact, or customer data |
| Security | Always. One line if nothing changes, the full section if it adds an entry point, a credential, or a permission, or exposes data to a new party |
| Validation | Always |
| Rollout and back-out | Always, except in an as-built doc |
| Open questions | Always |
| References | When there is anything to link |

The changelog is a separate file, `changelog.md`, shared by every version. Its template is at the end.

---

# <Title>

> Name the change: "Resume interrupted firmware downloads instead of restarting them". Name the component only if the doc introduces one, or describes one as-built: "Sync service". A reader should be able to guess the contents from the title.

| | |
|---|---|
| Author | |
| Team | <the team that owns what this design covers> |
| Reviewers | <names, and what you want from each of them> |
| Kind | Proposal \| As-built |
| Version | <1, 2... and one clause on why this version exists> |
| Key | <the ID prefix for the Values table, such as GATEWAY; only with a Values table> |
| Status | Draft \| In review \| Approved \| Superseded by v<n> |
| Last updated | <YYYY-MM-DD> |
| Review copy | <link to the copy on the review surface in docs/agents/docs.md, if it isn't the repo itself> |
| Requirements | <each requirements doc this design answers: its path or URL and the version> |
| Code | <the issues, repo, or branch that build it, once they exist> |
| Related | <prior docs, ADRs, tickets, specs, datasheets> |

> Status starts at Draft. Only a person sets Approved or Superseded.

## Summary

> Three sentences. What changes, why now, and what it costs. A reader who stops here should be able to repeat your decision back to you.

## Problem

> What is broken or missing today, and who feels it. Include the evidence: the bug, the field failure, the customer ask, the number. If you cannot point at something, say the problem is suspected and name the measurement that would confirm it.
>
> Then why now. A problem tolerated for a year needs a reason it stops being tolerable.

## Requirements

### Goals

> Each one testable. "Firmware update resumes after a dropped connection without re-sending bytes already written" is a goal. "Improve OTA reliability" is a wish.
>
> When a requirements doc exists, each goal starts with the ID of the requirement it answers and says what this design commits to for it: "SYS-4: the gateway forwards a theft event to the backend within 1 s of receiving it." The requirement's own text stays in the requirements doc. List only the requirements this design answers. When none exists, write the goals here.

### Values

Values other docs restate, which they cite by ID so a change here reaches them.

| ID | Value | Source |
|---|---|---|
| <KEY>-1 | <the value in words, with its number and unit: "Heartbeat: a check-in every 30 minutes"> | <decided (name, date), estimate, measured, datasheet, or config> |

> Only values another doc states. Number them from 1 in the Key from the header, and never renumber or reuse an ID. Where this doc restates a value in its prose, bind it to the ID: "every 30 minutes (GATEWAY-1)".

### Non-goals

> What you are deliberately not doing, with one clause on why. This is what reviewers try to add, so spend real thought here. Cite the ID when a non-goal is a Won't from the requirements doc.

## Constraints

> What cannot move, with the source of each. Hardware limits, power and memory budgets, protocol rules, existing wire contracts, deployment windows, a certification date. Cite the datasheet, spec section, measurement, or person, and a limit another of the project's docs sets by its requirement ID or linked section. A constraint nobody can attribute is often just a habit.
>
> A value your own team set and could change is a decision, not a constraint. It goes in Design, where it can be argued with.

## Architecture

> A system diagram, then each component in a line or two: what it is responsible for, and who it talks to. Then the data and control flow through the change, in order.
>
> Leave out the technology stack unless the change adds to it.

## Design

### How it works

> The mechanism, in order. Prose or a numbered walkthrough of the main path, then the edges.
>
> When the change reshapes more than one component, repeat this per component: responsibilities, inputs and outputs, error handling, key data structures, and where state lives. When it touches one, do not impose the structure on yourself.

### Interfaces and contracts

> Every boundary the change touches: message schemas, API shapes, object and resource IDs, database columns, file formats, config keys, CLI commands and flags. Show before and after. State explicitly what happens when a new peer meets an old one, and the reverse.

### State and data

> What is stored, where, who owns it, and what happens to what already exists. Include the migration if a schema moves, and whether it is reversible.

### Diagram

> A sequence or block diagram where it replaces a paragraph, not where it decorates one. Mermaid survives most review surfaces as a code block. Label arrows with real message names.

## Alternatives considered

> One short block per option someone genuinely wanted, including do-nothing. For each: what it was, and the specific reason it lost. If the reason is cost or time, say that rather than dressing it as a technical argument.

## Failure modes and risks

> For each part that can fail: what fails, how you detect it, what the system does next, and how far it spreads. A table works well.
>
> Cover the boring ones. Power loss mid-write, a peer on an old version, a message delivered twice, a clock that jumped, a queue that never drains, two copies running at once.

## Privacy

> If the change carries, stores, or exposes no identity, location, contact, or customer data, write one line naming the data the path does carry. "No change. The path carries device ID and retry count only." Otherwise answer the prompts below.
>
> What personal or confidential data does this path carry, store, or expose, and to whom? A location tied to a person is personal data.
>
> Where does it live, what is the retention, and how is deletion handled when someone asks? Name the legal obligation this affects if there is one, rather than asserting compliance in general.

## Security

> If the change adds no entry point, credential, or permission, and exposes no data to a new party, write one line saying so. "No change. No new endpoint, credential, or permission." Otherwise answer the prompts below.
>
> What is the new way in, and who can use it? Authentication and authorization for it, specifically, not the system's model in general.
>
> Which credentials or secrets are created, rotated, or expiring, and what is their lifetime. Which identity gains which permission, and is it the narrowest one that works.
>
> Then the threat model for what changed: who would want to abuse this, what they would gain, and what stops them. One paragraph on the actual change beats a page of generic attack classes.

## Validation

> How you know it works before the field does. Unit and integration coverage, hardware-in-the-loop, soak duration, pilot size, and the number that would make you roll back. Name quantities.

## Rollout and back-out

> The order things ship in, and whether each step deploys on its own. Call out explicitly when two sides must agree, for example that the server has to accept a new field before any client sends it.
>
> Then the back-out: what you do at 2am when this is the suspect. If a step cannot be undone, and flashing firmware or migrating data usually cannot, say so and name what stands in for it.

## Open questions

| Question | Owner | Needed by |
|---|---|---|
| | | |

> Unresolved is fine in a draft. Unowned is not.

## References

> Prior designs, ADRs, tickets, specs, datasheets, research, measurements, protocol traces. Anything a reader should be able to check but does not need to read.

---

# <Title>: changelog

Newest first. Each entry names the version it changed.

| Date | Version | Change | By |
|---|---|---|---|
