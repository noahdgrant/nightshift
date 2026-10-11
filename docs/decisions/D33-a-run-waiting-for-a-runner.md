---
id: D33
title: "A run waiting for a runner lock retries with backoff and gives up at its phase timeout"
issue: 99
date: 2026-10-10
amends: [D23]
---

# D33: A run waiting for a runner lock retries with backoff and gives up at its phase timeout

## Decision

Amends D23. A run waiting for a runner lock retries it non-blocking with a backoff of up to 5 s on the run's clock, and gives up at its own phase timeout, counted from when the wait began, or at `ns watch --until`, whichever is first. Giving up releases the locks already taken and logs `lock_wait_timeout`. The phase timeout counts as a failed attempt of that phase; `--until` ends the run as `budget`, and `ns watch` reports the night stopped by `until`. A lock taken at or past `--until` is given up the same way, and a budget spent by the time the locks are held ends the run as `budget` with the locks released, before the harness starts.

## Reason

A blocking `flock` had no bound: a hung holder, or a harness orphaned by a killed `ns`, kept the waiter blocked past its own timeout and past `--until` (#99). Polling on the `Clock` lets `NS_NOW` drive the wait in tests without real sleeping. Reusing `budget` for `--until` needs no new outcome or exit code: `ns watch` already returns the issue to the ready label and stops on `budget`, and names the stop `until` when the deadline has passed. A timeout as a failed attempt matches a timed-out harness (#117), and the retry gives a briefly stuck holder another chance. The budget is checked again once the locks are held, so the harness never starts on a spent budget, whatever the wait did to the clock or the spend. Spec: `docs/FACTORY.md` ("Runners").
