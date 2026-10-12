---
id: ADR-450
title: "ns-supervise names the night's report nightshift-<date>"
issue: 450
date: 2026-10-12
amends: [D63]
---

# ADR-450: ns-supervise names the night's report nightshift-<date>

## Decision

Amends D63. The supervisor's report is `<git-common-dir>/ns/handoffs/nightshift-<night>.md`, and its first line is `# nightshift-<night>`. `<night>` is still the local date the night started. A re-attaching supervisor continues the report under that name.

## Reason

The operator asked for it after supervising the night of 2026-10-11 (#450): a bare date says nothing about what the file is once it leaves the git dir. Reports under the old name are left as they are; the report is never committed, so nothing else reads them. Spec: `skills/ns-supervise/SKILL.md`, docs/factory/starting-a-night.md ("Supervising a night").
