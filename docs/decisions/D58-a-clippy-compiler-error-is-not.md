---
id: D58
title: "A clippy compiler error is not a finding"
issue: 289
date: 2026-10-10
amends: [D54]
---

# D58: A clippy compiler error is not a finding

## Decision

Amends D54 (#289). A compiler error from clippy, an error-level message with no code or an `E` code such as `E0308`, is not a finding: `check.sh clippy` exits 2 with no findings, so `ns rules check` fails as broken and `ns rules baseline` records nothing. The findings from the members that did compile, and the errors, go to stderr on `check.sh:` lines, which D50 doesn't read as findings. Lints with a code, clippy's and rustc's, stay findings.

## Reason

D54 made a compile error a `compile-error` finding, which a baseline could record. Cargo can't check a crate whose dependency didn't compile, so a baselined error hid every finding in the crates behind it, and they came back as new once it was fixed. D56 makes the C pack fail for the same reason, and the packs should handle it one way. Printing the other members' findings as text keeps `--keep-going`'s work visible without letting a broken run pass or be baselined. Spec: docs/FACTORY.md ("Rules").
