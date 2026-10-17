---
name: ns-tdd
description: Test-driven development with red-green-refactor. Use when building a feature or fixing a bug test-first, or when writing tests.
metadata:
  upstream: mattpocock/skills@b0618bc436ad:skills/engineering/tdd
---

# Test-Driven Development

The loop is red, then green. Every section applies on every cycle.

## What a good test is

A good test calls the code the way its users do and asserts what they observe. It survives a refactor because it never looks inside.

The **seam** is the public boundary a test sits at. Before the first test, write down the seams the work needs. Tests live at seams, never against internals.

## Rules of the loop

- **Red before green.** Write the failing test, watch it fail for the reason you expect, then write only enough code to pass it.
- **One slice at a time.** One seam, one test, one minimal implementation per cycle. Each test is a tracer bullet that answers what the last cycle taught you.
- **Every branch tested.** Each branch and error path you add gets a test that takes it, whether or not a requirement names it.
- **A bug fix starts with a test** that fails without the fix.

## Anti-patterns

- **Implementation-coupled**: mocks internal collaborators, tests private methods, or checks results through a side channel. The tell: the test breaks on a refactor that changes no behavior.
- **Tautological**: the expected value is computed the way the code computes it, so the test passes by construction. Take expected values from a literal, a worked example or the spec.
- **Horizontal slicing**: all tests first, then all code. Bulk tests describe imagined behavior. Work in vertical slices.

Mock only at system boundaries: network, disk, clock, hardware. Tests share no mutable state and no order.
