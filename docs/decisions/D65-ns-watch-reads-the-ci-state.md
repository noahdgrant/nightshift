---
id: D65
title: "ns watch reads the CI state of the default branch's head"
issue: 386
date: 2026-10-11
---

# D65: ns watch reads the CI state of the default branch's head

## Decision

`ns watch` reads the CI state of the default branch's head (#386), under `[queue] fix_main_first` (default true). Red is a check that failed while others still run, or a failed required check once all finish; green is every required check passed; pending and no CI are not red. The `Forge` trait's `main_checks` returns the head's checks, each with whether it is required; `watch::ci` classifies them. GitHub reads check runs, commit statuses and ruleset-required checks in one GraphQL query, and counts every check as required when no ruleset names one on the head. GitLab reads the head's commit statuses, with `allow_failure` meaning not required. `--dry-run` shows `main_ci` and `queue_held`; nothing holds the queue yet.

## Reason

Agreed in #377. Failing fast while checks still run catches a broken `main` before a slow job ends. A reader can't see classic branch protection, and a ruleset may require checks that run only on PRs (a title check), so required is what the head's checks and the rulesets agree on, else all. GitLab's commit statuses cover every job of the head's pipelines and external statuses in one list, so the trait needs no pipeline type. Spec: docs/factory/watch.md ("Main's CI").
