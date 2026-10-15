---
id: ADR-746
title: "The rules baseline is a ratchet on a finding's measured value"
issue: 746
date: 2026-10-15
amends: [D50, ADR-564]
---

# ADR-746: The rules baseline is a ratchet on a finding's measured value

## Decision

Amends D50 and ADR-564 (#746). For the rules whose `message` carries a number the finding measures, `ns` reads that number out of `message` by a table keyed by `(tool, rule)` in `cli/src/rules.rs` — today clippy's `clippy::too_many_lines` (`this function has too many lines (N/100) ...`) and cargo-deny's `duplicate` (`found N duplicate entries for crate 'X'`) — and the baseline is a ratchet on it. A matched finding whose value grew is `new` and exits 1, and the human line names both values (`grew from 221 to 261`); a value equal to or smaller than its entry's still matches. `ns rules check --prune` lowers a stored value that shrank and counts it in the new `lowered` JSON field; `ns rules baseline` treats a grown value as growth, refused without `--allow-growth`. `removal_only` reads a grown value as not removal-only, so the merge step keeps `.nightshift/rules-baseline.json`'s human-review hit for growth, while a shrunken value and a value-less message change stay removal-only. A rule the table lacks behaves exactly as before.

## Reason

A rule whose finding measures a size keeps the same `(tool, rule, file, snippet)` identity as it grows, because the value lives in the message, so `compare()` matched a grown finding to its baseline entry and passed: `cli/src/run.rs`'s `execute` grew 221 → 261 lines while matching its entry the whole time (#694's review, #744). The value is part of what the finding measures, so it belongs in the comparison. Reading it from `message` couples `ns` to a few rules' prose, which D50's reason avoided for whole tool formats; the finding format is unchanged, the table is the one place the list lives, and a rule it lacks is unaffected, which keeps the coupling small. Growth stays a human's call like any added finding, while a shrink is a refresh ADR-564 already lets through. The packs whose size findings carry no number (the Rust pack's `file-too-long`, java's and python's `file-size`, the JS pack's `max-lines` entry) can't be covered without changing the pack's message, filed as #769. Spec: docs/factory/run.md ("Rules"), docs/factory/merge.md (step 6).
