# Checks

The lens for the scan in [SKILL.md](../SKILL.md). Walk the hot spots organically and note where you feel friction; these groups name what to look for, and each one asks what the next agent would copy or get wrong.

## Vocabulary

Use these terms exactly, not "component", "service", "API" or "boundary".

- **Module**: anything with an interface and an implementation, at any scale: a function, a class, a package.
- **Interface**: everything a caller must know to use the module correctly: the signature, and also invariants, ordering, error modes and required configuration.
- **Depth**: behaviour a caller gets per unit of interface it learns. A **deep** module hides a lot behind a small interface; a **shallow** one has an interface nearly as complex as its implementation.
- **Seam**: the place where a module's interface lives, where behaviour can change without editing there.
- **Adapter**: a concrete thing that satisfies an interface at a seam. One adapter means a hypothetical seam; two mean a real one.
- **Leverage**: what callers get from depth. **Locality**: what maintainers get from it, with change and bugs concentrated in one place.
- **Deletion test**: imagine deleting the module. If its complexity vanishes, it was a pass-through. If it reappears across its callers, it earns its keep.

## One road per job

Each job (load config, raise an error, parse a duration, talk to the board) should have one way it is done, in one place. Look for:

- two live ways to do one job, both with callers: two config loaders, two HTTP clients, two error styles
- near-duplicate helpers, one written because the other "was awkward"
- an old path left alive "for compatibility" with no external caller

An agent copies whichever road it saw last, so the copies drift. Name both roads with their callers, and say which to keep.

## Road is the shortcut

A rule an agent has to read and remember is a rule it will break. Look for:

- rules held only by a comment, a README line or an `AGENTS.md` line that a type, lint, test or CI check could enforce: "always call X after Y", "add every new command to this list", "never import Z here"
- comments and lint or type suppressions papering over a bug, where the comment explains a defect instead of the code fixing it

Say which rung of the ladder would hold the rule, and what goes red when it is broken.

## Well-trodden stones

Agents do best on ground they have seen most. Look for:

- a dependency where the standard library or an existing dependency covers the job
- a home-made version of something the ecosystem has a standard for (an argument parser, a test runner, a retry loop)
- a pattern used nowhere else in the repo, or unusual for its language

A novel choice with a stated reason is fine. Report the ones with no reason.

## Navigability

An agent reads a file to change it, and a long or scattered one costs context and invites misses. Look for:

- files past about 1000 lines
- shallow modules: run the deletion test on each suspect, and report the ones whose complexity would vanish
- one concept spread across many small modules, so understanding it means bouncing between files
- pure functions pulled out only to test them, while the bugs hide in how they are called

## Verify loop

An agent needs a fast, scripted way to prove a change works on the real surface. Check `docs/agents/verify.md`:

- missing, or "not set up yet": a Strong finding whose fix is to run the `ns-setup-verify` skill
- it names a skill or CLI that doesn't exist, or a command that fails
- hot spots with no tests, or tests that only reach past the interface, so they break on every refactor
