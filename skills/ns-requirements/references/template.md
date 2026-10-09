# Requirements template

Lines beginning with `>` are prompts. Delete every one before the doc goes out.

| Section | Include |
|---|---|
| Summary | Always |
| User stories | Standalone and system docs, when a person uses the thing |
| Must, Should, Could, Won't | Always. An empty category gets one line saying so, not a missing heading |
| Open questions | Always |
| Sources | When any row cites outside research, a statute, a price, or a published list |

The changelog is a separate file, `changelog.md`, shared by every version. Its template is at the end.

A system doc's requirement tables add an *Allocated to* column between "Why / source" and "Verified by", naming the components that deliver each row.

---

# <Thing> requirements: <scope>

> "Meter requirements: city pilot". "CLI requirements: first public release". The title names the thing and the scope.

| | |
|---|---|
| Kind | Standalone \| System \| Component |
| Version | <1, 1.1, 2... and one clause on why this version exists> |
| Scope | <the release, feature, pilot, contract, or first version these priorities are relative to> |
| Key | <the ID prefix, e.g. METER> |
| Owner | <one person> |
| Status | Draft \| In review \| Approved \| Superseded by v<n> |
| Last updated | <YYYY-MM-DD> |
| Review copy | <link to the copy on the review surface in docs/agents/docs.md, if it isn't the repo itself> |
| Parent | <component docs only: the system doc> |
| Children | <system docs only: the component docs> |
| Related | <design docs, contracts, issues, prior requirement docs> |

> Status starts at Draft. Only a person sets Approved.

## Summary

> Two or three sentences: what the scope delivers and for whom, and what is deliberately left out. A reader who stops here knows what "done" means. A component doc also says what it owns that its system doc doesn't.

## User stories

> Numbered S1, S2, and so on, without a list marker. The actor is a person in a role. Cover every actor who touches the thing, including installers and support. Example: "S1. As a driver, I want to know when my parking time is about to run out, so that I can top up before I get a ticket."

S1. As a <actor>, I want <capability>, so that <benefit>.

Met as written in PARK, with no refinement here: <IDs>.

> Component docs only: every system row allocated to this component that no row below cites. IDs only, never the text.

## Must have

> The scope is not done without these. Example row: "PARK-4 | The driver receives an expiry warning at least 10 min before their paid time runs out | S1. 10 min estimated, to confirm with the pilot city | End-to-end test, 20 sessions, every warning at or before 10 min".

| ID | Requirement | Why / source | Verified by |
|---|---|---|---|
| <KEY>-<n> | | | |

## Should have

> Important, but the scope can be declared done without them.

| ID | Requirement | Why / source | Verified by |
|---|---|---|---|

## Could have

> Included if there is time.

| ID | Requirement | Why / source | Verified by |
|---|---|---|---|

## Won't have

> Not in this scope. The reason decides whether it comes back later, so give it.

| ID | Item | Why not now |
|---|---|---|

## Open questions

> One row per `[?]` in the doc, and anything else undecided.

| Question | Owner | Needed by |
|---|---|---|

## Sources

> Numbered, and cited from rows as [1], [2]. Each fact gets a link to the primary source and its date. When only a secondary source was reachable, link it and say so in the note, so no one mistakes it for primary. Keep facts the research found even when no row cites them yet.

| # | Fact | Source | Note |
|---|---|---|---|

---

# <Thing> requirements: changelog

Newest first. Each entry names the version it changed.

| Date | Version | Change | By |
|---|---|---|---|
