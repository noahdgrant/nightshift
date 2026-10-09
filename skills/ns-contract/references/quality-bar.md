# Quality bar

The bar a unit's diff must clear. `ns-build` checks its own diff against it before handing off, and every `ns-review` reviewer checks the diff against its axis's section. An unmet item is an Important finding unless the item says Critical. The review references under `ns-review/references/` say how to hunt for each one; this file says what must hold.

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
- Each condition and constant the diff adds is pinned: mutating it (flip a comparison, drop a guard, invert a condition) turns a test red.
- Mocks sit only at system boundaries (network, disk, clock, hardware). Tests share no mutable state or order.
- Test names read as specifications.

## Spec

- Every acceptance criterion in `brief.md` is implemented in full. A missing or wrong criterion is Critical.
- The diff adds nothing the brief didn't ask for: no new options, renamed public interfaces or drive-by fixes.

## Security

- Untrusted input is validated once, at the boundary where it enters.
- No untrusted value reaches a shell, SQL, `eval`, a file path or a template unescaped. An exploitable path is Critical.
- No secret sits in code, logs, errors, fixtures or committed config.
- Model output is treated as untrusted input.

## Architecture

- The change follows the patterns already in the code around it. A new pattern carries a reason.
- It reuses the existing helper for a job instead of adding a near-duplicate.
- Dependencies point the right way, with no cycles and no feature logic in shared modules.
- It adds no pass-through layer, and no `Optional`, `Any`, cast or silent fallback that hides an unclear invariant.
- An old path the change replaces is removed in the same change.
- No file grows past roughly 1000 lines.

## Readability

- Names say what a thing does or holds, in the repo's vocabulary.
- Control flow reads top to bottom: no flag arguments, no deep nesting, no nested conditional expressions.
- Nothing is dead: no unused code, parameters or imports, no commented-out code.
- No abstraction before its third use.
- The code meets every standards file the repo has (`CODING_STANDARDS.md`, `CONTRIBUTING.md`).

## Performance

- No query, request or file read inside a loop over results.
- Every loop, read and fetch over external data is bounded.
- No blocking call on an async or request path, and no quadratic scan where a set or map lookup works.

## Comments

- Comments say why, never what. Code that needs a comment to explain what it does is renamed or reshaped instead.
- No suppression (`# noqa`, `# type: ignore`, `#[allow]`, `// NOLINT`) hides a real bug.
- A constraint (`do not remove`) is a type, assertion, test or lint, not a comment.
