---
id: D50
title: "ns rules check runs the stack's rules commands against a baseline"
issue: 215
date: 2026-10-10
---

# D50: ns rules check runs the stack's rules commands against a baseline

## Decision

Builds on #215. `ns rules check` runs the commands in the `rules` row of `docs/agents/stack.md`, reads each output line that is a JSON object with `file` or `file:line[:col]: message` as a finding `{tool, rule, file, line, message, snippet}`, and fails only on findings `.nightshift/rules-baseline.json` doesn't list. A finding matches a baseline entry on `(tool, rule, file, snippet)`, where the snippet is the source line with whitespace collapsed; matching counts duplicates. `ns rules baseline` rewrites the baseline; it may only shrink unless `--allow-growth`, which is for a human. The build gate appends `ns rules check` when the row exists, and CI and `scripts/ci-local.sh` run it always; without a row it checks nothing and exits 0.

## Reason

Compose existing tools rather than write linters (#215, #233). Two input formats cover most tools: the compiler-style line most linters can print, and a JSON line for anything a short wrapper can convert, which also carries the rule id and lets a pack name its own `tool`. Reading per-tool formats in `ns` was rejected: every new tool would need a code change. The snippet, not the line number, is the identity, so an edit above a violation doesn't make it new; two violations on identical lines in one file need two entries, so a copied bad line still counts as new. A command that fails and prints no finding is an error, because a missing linter would otherwise pass as clean. A command that is killed, exits 126 or 127, runs over 300 s (`NS_RULES_TIMEOUT_MS`) or prints over 8 MiB (`NS_RULES_MAX_OUTPUT_BYTES`) is an error even with findings, because a cut or killed run would hide the rest. A missing baseline is empty, and creating one is growth, so the first baseline is a human's act too. Writing nothing when nothing was added or dropped keeps reruns from churning line numbers in a human-review file. CI runs the check without a row so the wiring lands before any pack; the gate only appends it when a row exists, so repos without one don't need `ns` on the gate's path. A repo whose `ci-local` gate already runs it (this one, once it has a row) runs the pack twice per gate. Spec: `docs/factory/run.md` ("Rules").
