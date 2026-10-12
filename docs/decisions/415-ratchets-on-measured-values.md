---
id: ADR-415
title: "Ratchets hold measured values to recorded ones, checked in the gate and CI"
issue: 415
date: 2026-10-12
---

# ADR-415: Ratchets hold measured values to recorded ones, checked in the gate and CI

## Decision

Measured values (binary size, flash and RAM use, coverage, warning counts) are ratchets, as agreed with the human in #105 on 2026-10-11. Each is a `[ratchets.<name>]` table in `nightshift.toml` with a `command` that prints one number, `better = "lower"` or `"higher"`, and a `tolerance` (a percentage of the recorded value, or an absolute number; default 0). The recorded values live in `ratchets.json` next to `nightshift.toml`, as `{"version": 1, "values": {...}}`. `ns ratchet check` fails a value worse than its recorded one by more than the tolerance, reports a ratchet with no recorded value as untracked without recording it, and refuses a command that fails or prints anything but one number. It runs each command twice and refuses, naming it, a ratchet whose two values differ by more than its tolerance; under `ns watch` only a ratchet's first check of the night does, tracked in the night directory. `ns run`'s gate runs it last whenever a ratchet is defined, and CI and `scripts/ci-local.sh` run it always. Only `ns ratchet update`, run by a human or a scheduled job, writes `ratchets.json`: it tightens and records, keeps a worse value unless `--allow-loosen`, and writes nothing when a ratchet is broken or flaky. Units never edit `ratchets.json` or a `[ratchets.*]` table.

The gate passes the check three environment variables rather than flags: `NS_RATCHET_FACTORY` (the run's factory directory, the night's snapshot under `ns watch`), `NS_RATCHET_STEADY` (the night's list of ratchets already run twice) and `NS_RATCHET_REPORT` (where the check copies its JSON). The report lets `ns run` name the ratchet, the measured and recorded values and the tolerance in build's feedback and in the stuck reason without parsing the gate's output.

## Reason

Rule violations already have a ratchet (`ns rules baseline`); measured values had none, so nothing deterministic noticed a unit growing an image or dropping coverage. Keeping the definition and the values in human-review files means a unit can only ever check against them; reading them from the run's factory directory, not the worktree, means a unit that edits them in its branch still faces the original line. `update` keeping worse values, even within tolerance, stops the tolerance from creeping the line one run at a time. The gate command is read once per night from the main checkout, before the night directory or a unit's log path exists, so the per-run paths go in through the environment `ns run` sets on the gate; `ns ratchet` removes them before running a ratchet's command so a nested `ns` doesn't inherit them. A metric that varies between two runs would make the gate pass or fail at random, so it fails loudly instead, and running it twice only once a night keeps a slow measurement from doubling every gate.
