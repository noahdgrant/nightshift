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
| `high` | `priority:high` | Blocks other open issues, breaks the factory or CI, or is a security issue |
| `medium` | `priority:medium` | Normal work |
| `low` | `priority:low` | Nice to have |

## States

| Role | Label in our tracker | Meaning | Next |
|---|---|---|---|
| `needs-triage` | `status:needs-triage` | New, not yet evaluated | `ns-triage` |
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
