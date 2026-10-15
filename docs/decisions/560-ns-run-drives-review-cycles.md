---
id: ADR-560
title: "ns run drives review cycles: one review pass or fix cycle per phase run"
issue: 560
date: 2026-10-14
amends: [D25, D27, D60]
---

# ADR-560: ns run drives review cycles: one review pass or fix cycle per phase run

## Decision

Amends D25, D27 and D60. `ns run`, not the review agent, drives the review loop. Each review pass and each fix cycle is its own phase run with its own timeout. A pass that leaves a Critical or Important open in changed code writes `review.md` with `status: fail`, `cause: findings_open`, `next: review` and the open IDs in `findings`. `ns run` then runs a fix cycle as a `review-fix` phase run, with `[phases.review]`'s skill, model, timeout and attempts. That run writes `fix.md` (`phase: review-fix`) listing the IDs it was handed, and the next pass updates `review.md` in place. `ns run` keeps the loop's record in `.ns/<unit>/review/cycles.json`: each pass and fix cycle with the times `ns` saw it start and end. The cycle count comes from that record, and the quality record carries it as `review_runs`. The limits are `[phases.review] full_cycles` (default 3) and `fix_only_cycles` (default 2); fix-only cycles hand over only findings with fewer than 2 attempts, as D60 set. Past the limits, `ns run` rewrites `review.md` as `blocked`, `next: human`, `cause: findings_open` with the open findings in its `reason`, and the unit ends stuck on that reason. A record that is missing or can't be parsed mid-loop stops the unit rather than count from 0, and a run whose entry `ns` can't write is set aside and stops it too.

## Reason

One session ran the whole loop under one phase timeout (#348), counted its own cycles, and wrote times that looked hand-made (#446). Ten units in the run log were blocked at the cycle limit, and each night showed a new edge in skill text that no test could reach. As phase runs, each step gets its own timeout and its own attempt accounting, routing lives in `core::step` where the sim and the fake-harness tests cover it, and a rerun resumes from the artifacts and the record rather than from a session that died. The limits D60 chose by hand from the quality records stay the defaults; they are now config. Eval cases: `fix-cycle-fixes-only-the-handed-finding` and `pass-after-fix-only-cycle-raises-no-new-finding`; the four cases that checked the skill blocking itself at the limit are retired, since `ns run` stops the loop (cargo tests in `cli/src/run/core/scenarios/review_loop.rs` and `cli/tests/factory/run_review_loop.rs`). Spec: skills/ns-review/SKILL.md, docs/factory/run.md.
