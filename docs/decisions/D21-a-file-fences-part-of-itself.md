---
id: D21
title: "A file fences part of itself for human review with ns:human-review lines"
date: 2026-10-08
---

# D21: A file fences part of itself for human review with ns:human-review lines

## Decision

A file can fence part of itself for human review with `ns:human-review` start and end lines (start takes an optional reason). The merge step asks for a human merge when a PR changes a line inside a region in either version, or changes a marker line.

## Reason

Path globs guard whole files, so a big file with one safety-critical table needed a human merge for every change. Substring detection works in any comment syntax. `ns check-markers` rejects unbalanced or nested markers. Spec: `docs/FACTORY.md`.
