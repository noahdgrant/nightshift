# review.md: judgment and format

## Judging findings

The reviewers saw only the diff and the contract. You have the repo and can trace call paths. Reviewers fill space with nits when they find nothing serious; if a reviewer's findings are all nits, the code is probably fine on that axis.

Classify each merged finding. The first matching row wins.

| Bucket | When | Status |
|---|---|---|
| Contract gap | The reviewer flagged it because `brief.md` was unclear or incomplete | `dismissed: contract gap: <what brief.md should say>`. List it under Contract gaps |
| Act on | A real defect in correctness, security or maintainability given the contract. In changed code it would block a real PR; in pre-existing code it is an escape | `open`, or `deferred: #<n>` once an escape is filed |
| Consider | Real, but you are unsure the fix is worth its cost now | `open`, severity Suggestion, with the trade-off stated |
| Noted | Valid but not actionable (premature, too low impact to track) | `dismissed: noted: <why>` |
| Dismissed | Wrong, a hypothetical the call path rules out, a preference, or missing context the repo supplies | `dismissed: <why>` |

Scrutinise a dismissal of a security or correctness finding twice. If Act on holds more than about five items, check whether you are filtering hard enough. The Dismissed list stays in the file: it lets the human override you.

## Severity mapping

| Source label | review.md severity |
|---|---|
| Critical; security Critical or High | Critical |
| Required; Important; security Medium; surviving mutation; missing test for new behaviour | Important |
| Optional, Consider, Nit, FYI; security Low or Info | Suggestion |

## Format

```markdown
---
unit: <unit-id>
phase: review
status: pass        # pass | fail | blocked
sha: <git rev-parse --short HEAD>
updated: <ISO-8601 UTC>
base: <base branch>@<sha>
head: <sha>
cycles: <n>         # fix cycles run, full and fix-only, 0 to 5
panel: full | reduced
launch: subagents | ns ask | inline (sequential)
---

# Review: <unit-id>

## Summary
Open: <n> Critical, <n> Important, <n> Suggestion. Fixed: <n>. Dismissed: <n>. Escapes filed: <n>.
Change size: <lines changed> lines in <n> files.
Panel: <full | reduced>, because <the size rule that chose it; for reduced, whether security was added and why>.

## Reviewers
| Reviewer | Role | Provider | Cycles run | Verdict | Findings |
|---|---|---|---|---|---|

## Critical
### C1. <title>
- Location: `path/to/file.py:42`
- Axis: correctness, security
- Scope: changed | pre-existing
- Cycle: 0
- Attempts: 1
- Raised by: correctness (provider-a), security (provider-b). Cross-provider.
- Finding: <one or two lines>
- Evidence: <trace, quote, or command + output>
- Status: open | fixed (cycle 2, <commit>) | dismissed: <reason> | deferred: #<issue>

## Important
### I1. ...

## Suggestion
### S1. ...

## Mutation check
| Mutation | Location | Command | Result |
|---|---|---|---|
| `>=` to `>` | `src/x.py:17` | `timeout 60 pytest -q tests/test_x.py` | killed |

## Contract gaps
- <what brief.md left unclear>

## Gaps
- <reviewer slice that produced no result, and why>
- <escape the tracker couldn't take: its ID and why. It doesn't block.>
```

Status follows the Gate in `SKILL.md`. For any `blocked` status, the first body line, before the title, is the one-sentence reason `ns watch` quotes. When the fix loop stops with findings open, it names the open Criticals and Importants in changed code, Criticals first:

```markdown
Open after at least 2 fix attempts: C1, I2, I4.

Options: fix by hand on the unit's branch then delete `.ns/<unit>/review.md` so `ns run` reviews again, split the issue, or route it to `ns-define`.

# Review: <unit-id>
```

Every finding, in `review.md` and in `review/cycle-<n>.md`, carries the fields in the template above, in that order; `ns watch` and `ns quality` parse them. Their values:

- `Axis`: the axes of the reviewers that raised it, from `correctness`, `readability`, `architecture`, `security`, `performance`, `tests`, `spec`, `comments`. Comma-separated when several raised it. The size Suggestions from step 1 take `architecture`.
- `Scope`: `changed` or `pre-existing`, as step 3 of `SKILL.md` defines them. `pre-existing` is what marks an escape. An escape sits in its severity's section with that section's ID prefix, like any finding (`### I4.` under `## Important`), since `ns quality` reads severity from the prefix. It gets no `## Escapes` section and no `E` prefix of its own.
- `Cycle`: the review pass that first raised it, `0` for the first review.
- `Attempts`: the fix cycles that handed it to the build agent, `0` until the first. The cycle it was fixed in is the one in `Status: fixed (cycle <n>, <commit>)`.
- `Status`: `deferred: #<n>` only for an escape filed as issue `#<n>`.

`Cycles run` is the review passes the reviewer ran in, for example `0, 1`.

Number findings once and keep the numbers stable across cycles, so the build agent and the human can refer to `I3`.

## Cycle files

Each fix cycle's record, `review/cycle-<n>.md`, opens with frontmatter, then lists the cycle's findings in the format above:

```markdown
---
cycle: <n>
kind: full | fix-only
started: <ISO-8601 UTC, when the build agent was launched>
ended: <ISO-8601 UTC, when the cycle's findings were judged>
---

## Important
### I3. ...
```

Cycle files from before fix-only cycles have no frontmatter and no `Attempts`; `ns quality` reads them without.
