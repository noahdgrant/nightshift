---
id: D30
title: "ns quality measures review outcomes from artifacts on disk"
issue: 147
date: 2026-10-09
amended_by: [D60]
---

# D30: ns quality measures review outcomes from artifacts on disk

## Decision

`ns quality` measures review outcomes from the artifacts already on disk: first-pass yield, first-pass Critical and Important findings per 100 changed lines by axis, cycles to clean, leftovers at the cycle limit, and escapes, with a trend by local day. It parses `review.md` leniently and reports what it couldn't parse instead of dropping it. An escape is D29's `pre-existing` finding: it is counted and blamed (`git blame` at the reviewed commit, where the finding's line number holds) to the PR that introduced the line, and like D29 it never counts against the unit, so it is left out of first-pass yield, findings per 100 lines, cycles to clean and leftovers.

## Reason

Without a number we couldn't tell whether build was getting better at passing review (#147). Reading the artifacts needs no new logging, and the run log adds review cost and outcome. Most existing artifacts predate D29's `Axis`, `Scope` and `Cycle` fields, so the command falls back to `Raised by`, `fixed (cycle 1, ...)` and the first pass's cycle file, and counts a unit with fix cycles as dirty when nothing says which pass raised a finding. Cycle-file ids are not matched against `review.md`: old artifacts reuse ids across passes. First-pass numbers use a unit's oldest review artifact; dismissed findings count toward nothing. The totals are higher than the issue's hand-counted baseline (357 findings against 214 on the same 25 units): a heading such as `### I1-I11 (cycle 1)` counts as eleven findings, and one-line `- S1. ...` suggestions count too, where the baseline counted only `###` headings. The `review.md` parser is shared with the `ns watch` stuck comment, which now also lists open findings written in those older shapes. Spec: `docs/factory/quality.md`. Amended by D60.
