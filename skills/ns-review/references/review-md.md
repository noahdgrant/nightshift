# review.md: judgment and format

## Judging findings

The reviewers saw only the diff and the contract. You have the repo and can trace call paths. Reviewers fill space with nits when they find nothing serious; if a reviewer's findings are all nits, the code is probably fine on that axis.

Classify each merged finding. The first matching row wins.

| Bucket | When | Status |
|---|---|---|
| Contract gap | The reviewer flagged it because `brief.md` was unclear or incomplete | `dismissed: contract gap: <what brief.md should say>`. List it under Contract gaps |
| Act on | A real defect in correctness, security or maintainability given the contract. It would block a real PR | `open` |
| Consider | Real, but you are unsure the fix is worth its cost now | `open`, severity Suggestion, with the trade-off stated |
| Noted | Valid but not actionable here (premature, low impact, outside the diff) | `dismissed: noted: <why>` |
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
cycles: <n>         # fix cycles run, 0 to 3
panel: full | reduced
launch: subagents | ns ask | inline (sequential)
---

# Review: <unit-id>

## Summary
Open: <n> Critical, <n> Important, <n> Suggestion. Fixed: <n>. Dismissed: <n>. Deferred: <n>.
Change size: <lines changed> lines in <n> files.
Panel: <full | reduced>, because <the size rule that chose it; for reduced, whether security was added and why>.

## Reviewers
| Reviewer | Role | Provider | Cycles run | Verdict | Findings |
|---|---|---|---|---|---|

## Critical
### C1. <title>
- Location: `path/to/file.py:42`
- Raised by: correctness (provider-a), security (provider-b). Cross-provider.
- Finding: <one or two lines>
- Evidence: <trace, quote, or command + output>
- Status: open | fixed (cycle 2, <commit>) | dismissed: <reason> | deferred: #<follow-up issue>

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
```

Status follows the Gate in `SKILL.md`. For any `blocked` status, the first body line, before the title, is the one-sentence reason `ns watch` quotes. For open Criticals it names them:

```markdown
Open Critical after 3 fix cycles: C1, C3.

Options: fix by hand on the unit's branch then delete `.ns/<unit>/review.md` so `ns run` reviews again, split the issue, or route it to `ns-define`.

# Review: <unit-id>
```

`Cycles run` is the review passes the reviewer ran in, for example `0, 1`.

Number findings once and keep the numbers stable across cycles, so the build agent and the human can refer to `I3`.
