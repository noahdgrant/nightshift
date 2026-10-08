---
name: sf-principles
description: Index of the sf-principle-* skills, with one line per principle saying when it applies. Read it when choosing which engineering principles bear on a task.
disable-model-invocation: true
---

# Principles index

Each principle is a user-invoked skill in `skills/sf-principle-<name>/`. Each line names when it applies. Read the full SKILL.md of any principle you apply, and in your report name the principle and the choice it changed.

From a skill directory, link a principle as `../sf-principle-<name>/SKILL.md`.

## Core

- [Laziness Protocol](../sf-principle-laziness-protocol/SKILL.md): refactoring, sizing a diff, or tempted to add abstractions, layers, or signal threading. Bias to deletion and the smallest change that solves the problem.
- [Foundational Thinking](../sf-principle-foundational-thinking/SKILL.md): before writing logic. Core types and data structures, scaffold-vs-feature sequencing, what concurrent actors share.
- [Redesign from First Principles](../sf-principle-redesign-from-first-principles/SKILL.md): integrating a new requirement into an existing design. Redesign as if it had been foundational from day one.
- [Attack the Premise](../sf-principle-attack-the-premise/SKILL.md): two or more fixes that share one premise have failed the same gate. Census the actors, then question the premise.
- [Subtract Before You Add](../sf-principle-subtract-before-you-add/SKILL.md): sequencing an addition, refactor, or rewrite. Remove dead weight first, then build on the simpler base.
- [Minimize Reader Load](../sf-principle-minimize-reader-load/SKILL.md): reviewing or shaping code that's hard to trace. Count layers and hidden state, collapse one-caller wrappers, shrink mutable scope.
- [Outcome-Oriented Execution](../sf-principle-outcome-oriented-execution/SKILL.md): planned rewrites and migrations with explicit phase boundaries. Converge on the target, skip throwaway compatibility states.
- [Experience First](../sf-principle-experience-first/SKILL.md): product, UX, API, or feature-scope tradeoffs. Choose user delight over implementation convenience.
- [Exhaust the Design Space](../sf-principle-exhaust-the-design-space/SKILL.md): a novel interaction or architectural decision with no precedent. Build 2-3 competing prototypes and compare before committing.
- [Build the Lever](../sf-principle-build-the-lever/SKILL.md): any non-trivial work. Build the script, codemod, or generator that does or proves it. The tool is the artifact a reviewer reruns.

## Architecture

- [Model the Domain](../sf-principle-model-the-domain/SKILL.md): writing stateful logic, or code that branches a lot or repeats a shape assumption across files. Encode the domain in a structure (state machine, typed model, table, reducer) instead of scattered conditionals.
- [Boundary Discipline](../sf-principle-boundary-discipline/SKILL.md): wiring validation, error handling, framework adapters, or a HAL. Guards at system boundaries, trust internal types, keep business logic pure.
- [Type System Discipline](../sf-principle-type-system-discipline/SKILL.md): designing types or a signature. Make illegal states unrepresentable, brand primitives, parse external data at boundaries.
- [Make Operations Idempotent](../sf-principle-make-operations-idempotent/SKILL.md): commands, lifecycle steps, boot or update paths, or loops that run amid crashes, power cuts, and retries. Converge to the same end state.
- [Migrate Callers Then Delete Legacy APIs](../sf-principle-migrate-callers-then-delete-legacy-apis/SKILL.md): introducing a new internal API while old callers exist. Migrate and delete in one wave.
- [Separate Before Serializing Shared State](../sf-principle-separate-before-serializing-shared-state/SKILL.md): concurrent actors (workers, worktrees, an ISR and the main loop) might write the same file, branch, key, or variable. Eliminate the sharing first.

## Verification

- [Prove It Works](../sf-principle-prove-it-works/SKILL.md): after a task, before declaring done. Verify against the real artifact, on target when hardware is involved, not a proxy or "it compiles".
- [Fix Root Causes](../sf-principle-fix-root-causes/SKILL.md): debugging. Reproduce first, ask why until you reach the cause, fix it there.
- [Sequence Work into Verifiable Units](../sf-principle-sequence-verifiable-units/SKILL.md): multi-step work and how you stack commits and PRs. Small units that each end in a check, verified before the next.
- [Test Behavior, Not Implementation](../sf-principle-test-behavior-not-implementation/SKILL.md): writing, changing, or keeping a test. Assert what users observe against a literal expected value. If it would pass with every import returning `None`, rewrite or delete it.
- [Explain the Number](../sf-principle-explain-the-number/SKILL.md): before you trust, report, or act on a measured number (speedup, regression, latency, eval result). Find what limits it and rule out that it measured something else.

## Delegation

- [Guard the Context Window](../sf-principle-guard-the-context-window/SKILL.md): context fills up with large outputs, long files, repeated reads, or fan-out planning. Route bulk to fresh-context agents, keep summaries in the main thread.
- [Never Block on the Human](../sf-principle-never-block-on-the-human/SKILL.md): tempted to ask "should I do X?" on reversible work. Proceed, present the result, let the human course-correct.

## Meta

- [Encode Lessons in Structure](../sf-principle-encode-lessons-in-structure/SKILL.md): you catch yourself writing the same instruction a second time. Encode it as a lint, metadata flag, runtime check, or script.
