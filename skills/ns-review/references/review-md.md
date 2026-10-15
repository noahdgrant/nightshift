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

A spec finding that a fix adds no guard for its class is dismissed when `build.md`'s Class guard section says why none is practical: `dismissed: no guard practical: <the reason>`. With no reason there, it stays open.

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
schema: ns-artifact/2
unit: <unit-id>
phase: review
status: pass        # pass | fail | blocked
cause: none         # none | findings_open | gate_red | criteria_failed | needs_human
next: ship          # ship on a pass; review for a fix cycle, build for a red check; human when blocked
reason: "<one line: the outcome, the open findings, what build must fix, or the blocker>"
findings: []        # under findings_open: the open IDs in changed code, Criticals first
sha: <git rev-parse --short HEAD>
updated: <ISO-8601 UTC>
base: <base branch>@<sha>
head: <sha>
cycles: <n>         # this pass's number: the fix cycles run before it
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

## Passes
| Pass | Panel | Reviewer | Role | Provider | Tokens | Seconds | Cost (USD) |
|---|---|---|---|---|---|---|---|
| 0 | reduced | correctness | review.correctness | claude | 48210 | 192 | 0.41 |
| 0 | reduced | spec | review.spec | codex | 30110 | 95 | |

## Critical
### C1. <title>
- Location: `path/to/file.py:42`
- Axis: correctness, security
- Scope: changed | pre-existing
- Cycle: 0
- Attempts: 1
- Raised by: correctness (provider-a), security (provider-b). Cross-provider.
- Dismissed raises: <raisers whose version you rejected; leave out when none>
- Missed by: <axis, escapes only>
- Finding: <one or two lines>
- Evidence: <trace, quote, or command + output>
- Status: open | fixed (cycle 2, <commit>) | dismissed: <reason> | deferred: #<issue>

## Important
### I1. ...

## Suggestion
### S1. ...

## Fallout
- fallout: <Suggestion ID>: `path/to/file.py:42`: <what is left>

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

Status, `cause` and `next` follow the Gate in `SKILL.md`. For any `blocked` status, `reason` is the one-sentence reason `ns watch` quotes. A pass that leaves findings for a fix cycle lists the open Criticals and Importants in changed code under `findings`, Criticals first, and `reason` names them:

```markdown
---
...
status: fail
cause: findings_open
next: review
reason: "Open in changed code: C1, I2, I4."
findings: [C1, I2, I4]
...
---
```

Every finding, in `review.md` and in `review/cycle-<n>.md`, carries the fields in the template above, in that order; `ns watch` and `ns quality` parse them. Their values:

- `Axis`: the axes of the reviewers that raised it, from `correctness`, `readability`, `architecture`, `security`, `performance`, `tests`, `spec`, `comments`. Comma-separated when several raised it. The size Suggestions from step 1 take `architecture`.
- `Scope`: `changed` or `pre-existing`, as step 3 of `SKILL.md` defines them. `pre-existing` is what marks an escape. An escape sits in its severity's section with that section's ID prefix, like any finding (`### I4.` under `## Important`), since `ns quality` reads severity from the prefix. It gets no `## Escapes` section and no `E` prefix of its own.
- `Cycle`: the review pass that first raised it, `0` for the first review.
- `Attempts`: the fix cycles that handed it to the build agent, `0` until the first. The cycle it was fixed in is the one in `Status: fixed (cycle <n>, <commit>)`.
- `Status`: `deferred: #<n>` only for an escape filed as issue `#<n>`.
- `Dismissed raises`: the raisers in `Axis` whose version of a finding you rejected while the finding stands on the others' evidence. Leave the line out when there are none. A dismissed finding needs none: all its raisers count as dismissed.
- `Missed by`: escapes only. The one axis whose reviewer should have caught it in the earlier review. `ns quality` counts an escape without it as unattributed.

`Cycles run` is the review passes the reviewer ran in, for example `0, 1`.

The Passes table is the denominator for every per-reviewer number `ns quality` reports, so it lists every worker of every pass, gaps rerun inline included:

- `Pass`: the review pass, `0` for the first.
- `Panel`: `full` or `reduced` for pass 0, as step 2 of `SKILL.md` chose; `cycle` for a full fix cycle's pass; `fix-only` for a fix-only cycle's.
- `Reviewer`: the worker's axis. `Role` is the `ns ask` role it ran under, or the fallback it used; `Provider` is the provider serving it, one word each.
- `Tokens`, `Seconds`, `Cost (USD)`: plain numbers, as the harness reported them for that worker. Leave a cell blank when the harness reported nothing; `0` reads as a free run.

Number findings once and keep the numbers stable across cycles, so the build agent and the human can refer to `I3`.

## Cycle files

Each review pass's record, `review/cycle-<n>.md` (`n` the pass number, `0` for the first), opens with frontmatter, then lists the findings the pass judged in the format above. `ns run` records when each pass and fix cycle ran in `review/cycles.json`; the cycle file carries no times.

```markdown
---
cycle: <n>
kind: first | full | fix-only
---

## Important
### I3. ...
```

Cycle files from before fix-only cycles have no frontmatter and no `Attempts`, and older ones carry `started` and `ended`; `ns quality` reads them either way.
