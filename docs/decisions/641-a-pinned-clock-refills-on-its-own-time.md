---
id: ADR-641
title: "On a pinned clock a free slot's refill counts the night's time, which its wait moves on, and each refill is logged"
issue: 641
date: 2026-10-14
amends: [D57]
---

# ADR-641: On a pinned clock a free slot's refill counts the night's time

## Decision

Amends D57. On a real clock nothing changes: a free slot beside running units is filled again every 60 seconds of real time. On a pinned clock (`NS_NOW`, tests only), the interval passes on the night's clock instead. Nothing else moves a pinned clock while units run, so the free slot's wait moves it on one second at each 50 ms poll, and the refill comes once the interval, rounded up to whole seconds, has passed on it. This happens only with `NS_REFILL_MS` set; a pinned night without it never refills and its clock stays put, as before in practice, since no test ran for 60 real seconds. Each refill logs a `refill` event with `at`, the night's clock in unix seconds, before its fill runs.

## Reason

The refill tests slept a fixed second or more and counted the `gh` calls made in it, so under load they measured the machine, not `ns watch` (#642, folded into #641). With the refill on the pinned clock, a test reads each refill's `at` and the calls after it from the run log, and checks that the interval passed between two refills and that each made one candidate listing and one queue read, without a sleep. A spinning slot shows as refills closer together than the interval on the night's clock, or more listings to one refill. Gating it on `NS_REFILL_MS` keeps the night's clock still in every other pinned test, where a moving clock would bring `--until` and the overlap limit closer. Spec: docs/factory/watch.md ("Parallel units").
