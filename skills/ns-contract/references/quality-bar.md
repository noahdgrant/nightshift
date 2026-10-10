# Quality bar

The bar a unit's diff must clear. `ns-build` checks its own diff against it before handing off, and every `ns-review` reviewer checks the diff against its axis's section. An unmet item takes the severity its tag gives, *(Critical)* or *(Suggestion)*. An untagged item takes the severity its axis reference under `ns-review/references/` gives that case (a security Low, a mutation that can't run on the host), and is Important when the reference gives none. The references also say how to hunt for each item; this file says what must hold.

Each item is checkable against the diff: met, or not met at a `file:line`, or not applicable with a one-line reason.

## Correctness

- Every path the diff adds handles empty input, `None`, zero, negative and boundary values the contract allows.
- Every error the diff can raise or receive is handled or propagated on purpose. Nothing is swallowed (`except: pass`, an ignored `Result`).
- Comparisons and ranges are right at the boundary: `<` versus `<=`, first and last element, off-by-one.
- State changes are all-or-nothing: a failure partway leaves state as it was or clearly marked.
- Running the operation twice, or after a crash halfway, is safe or refused.
- Shared state is guarded by structure (a lock, ownership, sequencing), not by a convention.
- A guard, retry or cast fixes the cause, not the symptom ([fix root causes](../../ns-principle-fix-root-causes/SKILL.md)).

## Tests

- Every behaviour the diff adds or changes has a test, and so does every branch and error path it adds, not only the acceptance criteria.
- A bug fix has a test that fails without the fix.
- Tests go through the public interface and assert literal expected values ([test behaviour, not implementation](../../ns-principle-test-behavior-not-implementation/SKILL.md)).
- Each condition and constant the diff adds is pinned: mutating it (flip a comparison, drop a guard, invert a condition) turns a test red. A mutation that can't run on the host is `inconclusive`, never killed.
- Mocks sit only at system boundaries (network, disk, clock, hardware). Tests share no mutable state or order.
- Test names read as specifications. *(Suggestion)*

## Spec

- Every acceptance criterion in `brief.md` is implemented in full. *(Critical)*
- The diff changes no behaviour callers can see beyond what the brief asked for: no new options, renamed public interfaces or drive-by fixes.
- Internal tidy-ups the brief didn't ask for go in their own change. *(Suggestion)*

## Security

- Untrusted input is validated once, at the boundary where it enters.
- No untrusted value reaches a shell, SQL, `eval`, a file path or a template unescaped. *(Critical when exploitable)*
- No secret sits in code, logs, errors, fixtures or committed config. *(Critical)*
- Model output is treated as untrusted input.

## Architecture

- The diff adds no second way to do a job the repo already does one way: it follows the patterns in the code around it and reuses the existing helper instead of adding a near-duplicate ([pave the road](../../ns-principle-pave-the-road/SKILL.md)).
- A new dependency, tool or pattern states why the existing or conventional one doesn't fit ([pave the road](../../ns-principle-pave-the-road/SKILL.md)).
- Dependencies point the right way, with no cycles and no feature logic in shared modules.
- It adds no pass-through layer, and no `Optional`, `Any`, cast or silent fallback that hides an unclear invariant.
- An old path the change replaces is removed in the same change.

## Readability

- Names say what a thing does or holds, in the repo's vocabulary. *(Suggestion)*
- Control flow reads top to bottom: no flag arguments, no deep nesting, no nested conditional expressions.
- Nothing is dead: no unused code, parameters or imports.
- No abstraction before its third use.
- The code meets every standards file the repo has (`CODING_STANDARDS.md`, `CONTRIBUTING.md`).

## Performance

- No query, request or file read inside a loop over results.
- Every loop, read and fetch over external data is bounded.
- No blocking call on an async or request path, and no quadratic scan where a set or map lookup works.

## Size

The review lead checks this once, in its sizing step, so no reviewer gets this section.

- The unit stays under the soft limit in `docs/agents/stack.md` and keeps a refactor apart from new behaviour. *(Suggestion)*
- No file grows past roughly 1000 lines. *(Suggestion)*

## Comments

- Code that needs a comment to explain what it does is renamed or reshaped instead.
- No narrating comment, banner or commented-out code. *(Suggestion)*
- No suppression (`# noqa`, `# type: ignore`, `#[allow]`, `// NOLINT`) hides a real bug.
- A constraint (`do not remove`) is a type, assertion, test or lint, not a comment.
