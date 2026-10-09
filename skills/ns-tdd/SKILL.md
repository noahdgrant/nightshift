---
name: ns-tdd
description: Test-driven development with red-green-refactor. Use when building a feature or fixing a bug test-first, or when writing integration tests.
metadata:
  upstream: mattpocock/skills@b0618bc436ad:skills/engineering/tdd
---

# Test-Driven Development

TDD is the red → green loop. This skill is the reference that makes that loop produce tests worth keeping: what a good test is, where tests go, the anti-patterns, and the rules of the loop. Every section applies on every cycle: consult them before and during the loop, not after.

Read `docs/agents/stack.md` for the test commands and the test seams this repo has. If it is missing, load the `ns-setup` skill instead of guessing. When exploring the codebase, read the glossary that `docs/agents/domain.md` points to, so test names and interface vocabulary match the project's domain language, and respect ADRs in the area you're touching.

## What a good test is

Tests verify behavior through public interfaces, not implementation details. Code can change entirely; tests shouldn't. A good test reads like a specification: "user can checkout with valid cart" tells you exactly what capability exists, and it survives refactors because it doesn't care about internal structure. This is [test behavior, not implementation](../ns-principle-test-behavior-not-implementation/SKILL.md): apply it to every test you write, change or keep.

See [tests.md](references/tests.md) for examples and [mocking.md](references/mocking.md) for mocking guidelines.

## Seams: where tests go

A **seam** is the public boundary you test at: the interface where you observe behavior without reaching inside. Tests live at seams, never against internals.

**Test only at pre-agreed seams.** Before writing any test, write down the seams under test and confirm them with the user. No test is written at an unconfirmed seam. You can't test everything, so agreeing the seams up front is how testing effort lands on the critical paths and complex logic instead of every edge case. Under `gates: auto`, with no human to confirm, take the seams from the brief's acceptance criteria and record them in the phase artifact.

Ask: "What's the public interface, and which seams should we test?" Give each proposed seam a one-line note on what it catches and what it misses.

**Firmware.** Code that touches hardware has a ladder of seams: host-compiled unit tests behind a hardware abstraction layer, a simulator or emulator, on-target tests, and hardware-in-the-loop. Each catches different defects at a different cost. Read [firmware-seams.md](references/firmware-seams.md) when the code under test reaches registers, interrupts, peripherals or timing.

## Anti-patterns

- **Implementation-coupled**: mocks internal collaborators, tests private methods, or verifies through a side channel (querying the database instead of using the interface). The tell: the test breaks when you refactor but behavior hasn't changed.
- **Tautological**: the assertion recomputes the expected value the way the code does (`assert add(a, b) == a + b`, a snapshot derived by hand the same way, a constant asserted equal to itself), so it passes by construction and can never disagree with the code. Expected values must come from an independent source of truth: a known-good literal, a worked example, the spec, a datasheet.
- **Horizontal slicing**: writing all tests first, then all implementation. Bulk tests verify _imagined_ behavior: you test the _shape_ of things rather than user-facing behavior, the tests go insensitive to real changes, and you commit to test structure before understanding the implementation. Work in **vertical slices** instead: one test → one implementation → repeat, each test a **tracer bullet** that responds to what the last cycle taught you. This is [sequence verifiable units](../ns-principle-sequence-verifiable-units/SKILL.md) at the scale of one test.

## Rules of the loop

- **Red before green.** Write the failing test first and watch it fail for the reason you expect. Then write only enough code to pass it. Don't anticipate future tests or add speculative features.
- **One slice at a time.** One seam, one test, one minimal implementation per cycle.
- **Every branch tested.** Each branch and error path you add gets a test that takes it, not only the paths the acceptance criteria name. This is the Tests section of the [quality bar](../ns-contract/references/quality-bar.md), the list review checks against.
- **Refactoring is not part of the loop.** It belongs to the review stage (see the `ns-review` skill), not the red → green implementation cycle.
