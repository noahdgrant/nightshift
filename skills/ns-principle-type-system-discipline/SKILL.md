---
name: ns-principle-type-system-discipline
description: "Apply when designing types, reviewing a function signature, or writing code in any statically-typed language. Make illegal states unrepresentable, brand semantic primitives, parse external data at boundaries, refuse to lie to the compiler, exhaust variants, derive from authoritative schemas."
disable-model-invocation: true
metadata:
  upstream: cursor/plugins@ccb5507cec15:pstack/skills/principle-type-system-discipline
---

# Type System Discipline

The type checker is a proof assistant. Use it to eliminate impossible states, mismatched primitives, and unhandled variants before the code runs. A case the types let you ignore becomes a runtime failure the checker could have stopped. Prefer defining errors and special cases out of existence over proliferating handlers. Unrepresentable states, total functions, and interface redesign (the patterns below) are the tools.

Applies to any typed language. In Python, annotated code checked strictly by mypy or pyright plays the compiler's role.

**The patterns:**

- **Make illegal states unrepresentable.** Model variants as sum types: a `Union` of frozen dataclasses in Python, enums with payloads in Rust, tagged unions in C. A bag of optional fields lets contradictory combinations type-check. A subtle anti-pattern: `completed: bool` plus `completed_at: datetime | None` admits `completed=True, completed_at=None`, which is meaningless. Derive the boolean from one source (`completed_at is not None`), or model the variants as `Open | Done(at: datetime)`. If a bug forces the question "wait, can this combination actually happen?", the type is too loose.
- **Types are constructions, not restrictions.** Build the type up from the values you want instead of carving them out of a looser type with checks. A non-empty list is a head plus a rest, not a list with a length check. A valid time range is a start plus a duration, not two timestamps you must keep ordered. Choose the shape that cannot build the illegal value and expose the interface callers need on top.
- **Brand semantic primitives.** `UserId` and `OrderId` are strings underneath but should not be interchangeable. Use `typing.NewType` (or a newtype in Rust). Validate once at creation, trust the type downstream.
- **External data is untyped until parsed.** RPC payloads, JSON, IPC messages, CLI args, config files, environment variables, database rows, serial frames. Have a parse function at every boundary that turns unstructured input into the typed model. See [Boundary Discipline](../ns-principle-boundary-discipline/SKILL.md) for where to put validation.
- **Tell the type checker the truth.** `typing.cast`, `Any`, and `# type: ignore` are latent runtime crashes. If the checker can't prove a fact, prove it (validate, narrow with `isinstance`, refine the model) or accept that the cast is a hazard.
- **Exhaustive matching is the checker's job.** When you match on a sum type, a new unhandled variant must fail the type check. In Python, end the `match` with `case _: assert_never(x)`. In Rust, use an unannotated `match`. In C, enable `-Wswitch-enum`.
- **Derive types from authoritative schemas.** When a protocol buffer, OpenAPI spec, database migration, or register description (SVD) defines a shape, generate from it instead of hand-rolling a parallel type. See [Encode Lessons in Structure](../ns-principle-encode-lessons-in-structure/SKILL.md).
- **Strengthen a type only where partiality appears.** A runtime assertion, `None` check, or "this should never happen" raise marks the place a type is too weak. Push that check up into the type. Then stop. The type system's job is to track the cases each use site must handle, not to describe the data as precisely as possible. Prefer total functions. `sum` of an empty list is 0, so it takes the plain list. The first item of an empty list has no answer, so it demands the non-empty one.

**Firmware:** give units and register fields their own types (`Millivolts`, `TimerTicks`, an enum per peripheral mode), so a raw integer cannot flow into the wrong register.

**The tests:**

- "Can I write a comment explaining when this combination of fields is valid?" If yes, the type is too loose. Split it into a sum type.
- "Do two of my function arguments share a primitive type but mean different things?" Brand them.
- "Where did this `Any`, this `cast`, this `# type: ignore` come from?" Trace it to the boundary and validate there instead.
- "If a new variant is added next month, will the checker tell the next agent where to add a case?" If no, the match isn't exhaustive.
- "Is this type duplicating a shape another file owns?" Derive instead.
- "Am I strengthening this type to keep an operation total, or just to be more precise?" If nothing would otherwise fail, keep the plain type.
