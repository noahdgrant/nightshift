# Triage Labels

The `ns-*` skills speak in canonical triage roles. This file maps each role to the label string this repo's tracker uses. Every triaged issue carries exactly one category, one priority and one state.

## Categories

| Role | Label in our tracker | Meaning |
|---|---|---|
| `bug` | `bug` | Something is broken |
| `enhancement` | `enhancement` | New feature or improvement |

## Priorities

| Role | Label in our tracker | Meaning |
|---|---|---|
| `high` | `priority:high` | Blocks other open issues, breaks the factory or CI, or is a security issue |
| `medium` | `priority:medium` | Normal work |
| `low` | `priority:low` | Nice to have |

## States

| Role | Label in our tracker | Meaning | Next |
|---|---|---|---|
| `needs-triage` | `needs-triage` | New, not yet evaluated | `ns-triage` |
| `needs-info` | `needs-info` | Waiting on the reporter for more information | reporter, then `ns-triage` |
| `needs-repro` | `needs-repro` | A bug with no reproducible failure yet | `ns-troubleshoot` |
| `needs-define` | `needs-define` | Intent unclear, or the change is large | `ns-define` |
| `ready-for-agent` | `ready-for-agent` | Agent Brief written, small enough to build directly | `ns-build` |
| `ready-for-human` | `ready-for-human` | Brief written, but needs a human (hardware, a judgement call, credentials) | a person |
| `wontfix` | `wontfix` | Will not be actioned | none |

When a skill mentions a role (e.g. "apply the `ready-for-agent` label"), use the corresponding string from these tables.

`ns watch` also uses two run states. Skills never set these:

| Label | Meaning |
|---|---|
| `in-progress` | `ns run` is working on the issue |
| `in-review` | nightshift opened a PR, waiting for human review |
