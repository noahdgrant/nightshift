---
id: ADR-433
title: "The Java pack flags catch (Throwable) with AvoidCatchingGenericException, not the deprecated AvoidCatchingThrowable"
issue: 433
date: 2026-10-13
amends: [ADR-367]
---

# ADR-433: The Java pack flags catch (Throwable) with AvoidCatchingGenericException

## Decision

Amends ADR-367. `rules/java/ruleset.xml` runs PMD's `AvoidCatchingGenericException` with `typesThatShouldNotBeCaught` set to `java.lang.Throwable` only, in place of `AvoidCatchingThrowable`, and the finding's rule is `AvoidCatchingGenericException`. `rules/java/tests/run.sh` fails when PMD warns that the ruleset uses a rule scheduled for removal.

## Reason

PMD 7.18 deprecated `AvoidCatchingThrowable` and PMD 8 removes it, so ADR-367 kept it only until a PMD 8 bump. Swapping now takes the step out of that bump and stops the warning on every run. The rule's default list also has `Exception`, `RuntimeException`, `NullPointerException` and `Error`; narrowing it to `Throwable` keeps the pack flagging what it flagged before, since catching `Exception` at a boundary is often on purpose. `check.sh` sends PMD's log nowhere a person reads, which is how the warning went unseen, so `run.sh` checks for it. A repo whose baseline lists `AvoidCatchingThrowable` findings sees them as new after it reinstalls the pack, and rebaselines them by hand under their new rule name.
