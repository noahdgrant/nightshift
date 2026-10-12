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

- **Patterns**: does the change follow existing patterns? A second way to do a job the repo already does one way is a finding, and a new pattern needs a reason ([pave the road](../../ns-principle-pave-the-road/SKILL.md)).
- **Module boundaries**: dependencies flowing the right way, no cycles, feature logic kept out of shared or general-purpose modules.
- **Canonical helpers**: a bespoke near-duplicate of an existing helper is a finding. Name the helper to reuse.
- **Complexity relocated, not reduced**: count the concepts a reader must hold. If a "cleaner" version keeps that count, it isn't cleaner. Prefer the restructuring that makes branches, modes or layers disappear.
- **Shallow modules** and pass-through layers added by the diff.
- **Type boundaries**: needless `Optional`, `Any`, casts, or silent fallbacks papering over an unclear invariant. Validate once at the boundary, then trust the value inside.
- **Bolted-on vs integrated**: if the requirement had been known from the start, would the code look like this?
- **Legacy dual paths**: a new API added while the old one stays alive with no external consumer. Migrate callers and delete the old path in the same change.
- **Interface changes without a contract review**: the diff changes something a caller outside the diff can observe (output format or field names, ordering, error text, exit codes, flags, a schema, a wire, register or persisted format), and neither the brief nor `build.md` decides it: no old fields kept, no version, no line saying who it breaks. Name what changed and who can observe it ([Hyrum's law](../../ns-principle-hyrums-law/SKILL.md), [contract first](../../ns-principle-contract-first/SKILL.md)).
- **Non-atomic updates** that can leave state half-applied, and independent work serialised for no reason.
- **Dependencies**: a new dependency the standard library or existing code already covers, or a new dependency or tool with no stated reason. For an upgrade: changelog read, one dependency per change, lockfile diff reviewed.
- **File growth**: a file pushed past roughly 1000 lines. Decompose first, then add. A Suggestion, as the bar's Size section says.

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

## TypeScript and React

Apply this section only when the diff touches `.ts`, `.tsx`, `.js` or `.jsx` files. Each item names its upstream source.

- **Bag of optional fields**: a type whose variants are spread over optional fields (`{ loading: boolean; data?: T; error?: string }`) compiles contradictory combinations. Model the variants as a discriminated union on one literal field (`kind`, `type` or `tag`, whichever the repo uses). Source: pstack `typescript-best-practices` (Discriminated unions).
- **Illegal states representable**: when a bug would make you ask "can this combination happen?", the type is too loose. Build the shape so the bad value can't be constructed: `[T, ...T[]]` for non-empty, `start` plus `duration` for a range, a boolean derived from the one field that decides it. Source: pstack `principle-type-system-discipline`, `typescript-best-practices` (Constructive modeling).
- **Hand-rolled guards at the boundary**: external data (`JSON.parse`, fetch, IPC, env, database rows) is `unknown` until a schema parses it. Parse once where it enters with the schema library the repo already uses, and infer the type from the schema (`z.infer`). A property-by-property guard beside a duplicate interface is the finding. Source: pstack `typescript-best-practices` (Schemas before guards, Boundary validation).
- **Copied shapes**: a new interface restating fields another type, a generated type or a schema already defines. Derive it with `Pick`, `Omit`, `Parameters`, `ReturnType`, `Awaited` or `typeof`. Source: pstack `typescript-best-practices` (Schema-derived types).
- **Mixable IDs**: two IDs of the same primitive type passed side by side where swapping them would compile and do harm. Brand them (`string & { readonly __brand: "OrderId" }`), validated once at creation. Brand only where the mix-up is a real risk. Source: pstack `typescript-best-practices` (Branded types).
- **Positional parameters and boolean flags**: several same-typed positional arguments, or `doThing(true, false)`, hide meaning at the call site. Pass one object argument, or split the function. Hot paths (per-frame render, parsers) may keep positional arguments. Source: pstack `typescript-best-practices` (Object args), addyosmani `code-simplification`.
