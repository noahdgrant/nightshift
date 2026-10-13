---
id: ADR-454
title: "The ns-contract scratch-file rule has no eval case; ns lint enforces it"
issue: 454
date: 2026-10-13
---

# ADR-454: The ns-contract scratch-file rule has no eval case; ns lint enforces it

## Decision

The `ns-contract` rule that a phase never writes a fixed `/tmp/<name>` path (Scratch files) has no eval case. `ns lint` fails any skill that names such a path (`fixed_tmp_paths`), and `ns run` gives each phase its own `TMPDIR`, which the CLI tests cover (`cli/tests/factory.rs`).

## Reason

`ns-contract` is installed as an `extra_skills` dependency in the eval arms, not as a skill under test, so a case would measure whether a model follows one sentence it can break only by writing a fixed path. The lint rule catches that deterministically in skill text, and the per-phase `TMPDIR` removes the collision itself. A case would test nothing the lint and the CLI tests do not.
