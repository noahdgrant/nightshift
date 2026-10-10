# Architecture reviewer

What must hold on this axis is the **Architecture** section of the [quality bar](../../ns-contract/references/quality-bar.md). This file is how to find where it doesn't.

Does the change fit the system's design, and does it leave the structure better or worse?

## Vocabulary

Use these terms exactly.

- **Module**: anything with an interface and an implementation, at any scale.
- **Interface**: everything a caller must know: signature, invariants, ordering, error modes, configuration, performance.
- **Deep** module: much behaviour behind a small interface. **Shallow**: the interface is nearly as complex as the implementation.
- **Seam**: the place where a module's interface lives, where behaviour can change without editing there.
- **Locality**: change, bugs and knowledge concentrate in one place.

## Look for

- **Patterns**: does the change follow existing patterns? A new pattern needs a reason.
- **Module boundaries**: dependencies flowing the right way, no cycles, feature logic kept out of shared or general-purpose modules.
- **Canonical helpers**: a bespoke near-duplicate of an existing helper is a finding. Name the helper to reuse.
- **Complexity relocated, not reduced**: count the concepts a reader must hold. If a "cleaner" version keeps that count, it isn't cleaner. Prefer the restructuring that makes branches, modes or layers disappear.
- **Shallow modules** and pass-through layers added by the diff.
- **Type boundaries**: needless `Optional`, `Any`, casts, or silent fallbacks papering over an unclear invariant. Validate once at the boundary, then trust the value inside.
- **Bolted-on vs integrated**: if the requirement had been known from the start, would the code look like this?
- **Legacy dual paths**: a new API added while the old one stays alive with no external consumer. Migrate callers and delete the old path in the same change.
- **Non-atomic updates** that can leave state half-applied, and independent work serialised for no reason.
- **Dependencies**: a new dependency the standard library or existing code already covers. For an upgrade: changelog read, one dependency per change, lockfile diff reviewed.
- **File growth**: a file pushed past roughly 1000 lines. Decompose first, then add.

## Smell baseline

Judgement calls, overridden by any documented repo standard:

- **Feature Envy**: a function reaching into another object's data more than its own. Move it to the data.
- **Data Clumps**: the same few fields or parameters travelling together. Give them one type.
- **Primitive Obsession**: a string or int standing in for a domain concept. Give the concept a small type.
- **Repeated Switches**: the same `if`/`match` cascade on the same type in several places. One mapping or polymorphism.
- **Shotgun Surgery**: one logical change forces scattered edits. Gather what changes together.
- **Divergent Change**: one module edited for several unrelated reasons. Split it.
- **Message Chains**: `a.b().c().d()` walks the caller shouldn't depend on. Hide the walk behind one method.
- **Refused Bequest**: a subclass ignoring most of what it inherits. Use composition.

Do not penalise simple code for lacking abstraction. Three lines of duplication beat a premature abstraction.

Firmware: check the layering between hardware access (registers, HAL), drivers and application logic. Application code poking registers directly, or a driver holding business rules, is a boundary finding. Code that must run on host and target should sit behind a seam the host tests can use.
