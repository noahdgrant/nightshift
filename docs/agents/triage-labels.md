# Triage Labels

The `ns-*` skills speak in canonical triage roles. This file maps each role to this repo's GitHub labels. Every triaged issue carries exactly one category, one priority and one state.

## Categories

Categories are the Conventional Commit type labels.

| Role | Label in our tracker | Meaning |
|---|---|---|
| `bug` | `type:fix` | Something is broken |
| `enhancement` | `type:feat` (or `type:docs`, `type:refactor`, `type:test`, `type:ci`, `type:chore` when that fits better) | New feature or improvement |

## Priorities

`ns watch` works higher priorities first, then sorts by category.

| Role | Label in our tracker | Meaning |
|---|---|---|
| `critical` | `priority:critical` | Fix next: a Critical or security finding, including one that escaped an earlier review |
| `high` | `priority:high` | Blocks other open issues, or breaks the factory or CI |
| `medium` | `priority:medium` | Normal work |
| `low` | `priority:low` | Nice to have |

## Areas

Every issue also carries at least one area label.

| Label | Covers |
|---|---|
| `area:cli` | the `ns` CLI in `cli/` |
| `area:skills` | the skills under `skills/` |
| `area:evals` | eval cases, fixtures and `ns eval` |
| `area:factory` | the overnight factory: `ns watch`, `ns run`, `.nightshift/` |
| `area:repo` | repo-wide docs and config |

## States

| Role | Label in our tracker | Meaning | Next |
|---|---|---|---|
| `needs-triage` | `status:needs-triage` | New, not yet evaluated | `ns-triage` (`ns watch` triages these, and issues with no state, before each unit) |
| `needs-info` | `status:needs-info` | Waiting on the reporter for more information | reporter, then `ns-triage` |
| `needs-repro` | `status:needs-repro` | A bug with no reproducible failure yet | `ns-troubleshoot` |
| `needs-define` | `status:needs-define` | Intent unclear, or the change is large | `ns-define` |
| `ready-for-agent` | `status:ready-for-agent` | Agent Brief written, small enough to build directly | `ns-build` (picked up by `ns watch`) |
| `ready-for-human` | `status:ready-for-human` | Needs a human (a judgement call, credentials, a stuck run) | a person |
| `wontfix` | `status:wontfix` | Will not be actioned; close as not planned | none |

`ns watch` also uses two run states. Skills never set these:

| Label | Meaning |
|---|---|
| `status:in-progress` | `ns run` is working on the issue |
| `status:in-review` | nightshift opened a PR, waiting for human review |

When a skill mentions a role (e.g. "apply the `ready-for-agent` label"), use the corresponding string from these tables.
