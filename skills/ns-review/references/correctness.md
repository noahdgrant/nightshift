# Correctness reviewer

What must hold on this axis is the **Correctness** section of the [quality bar](../../ns-contract/references/quality-bar.md). This file is how to find where it doesn't.

Does the code do what it claims, on every path the contract covers?

## Look for

- **Edge cases**: empty input, `None`, zero, negative, boundary values, the largest supported size.
- **Error paths**: errors caught, propagated, or silently swallowed. A bare `except:` or `except Exception: pass` hides failures.
- **Off-by-one**, integer overflow and truncation, encoding, float comparison.
- **State**: race conditions, stale or shared mutable state, half-applied updates that leave state inconsistent.
- **Idempotency**: what happens if the operation runs twice, or a previous run crashed halfway?
- **Concurrency**: is shared state serialized by structure (locks, ownership, sequential phases) or by a convention that won't hold?
- **Root cause vs symptom**: a guard that masks a broken invariant, a retry that hides a broken contract, a cast that silences a modelling error. Ask what the proper fix at the right layer looks like. See [fix root causes](../../ns-principle-fix-root-causes/SKILL.md).
- **Happy and sad path**: walk both through the diff.

Trace the execution path for every potential bug. Show the call chain that produces the bad value.

## Mutation check

Tests that pass are not proof they would catch a regression. Answer by experiment.

Pick up to five conditions or constants the diff adds or changes that the tests should pin down: drop a negation, swap `and` for `or`, change `>=` to `>`, return early, replace a constant. For each, name the test you expect to go red.

- If you can write files and run commands, run each mutation yourself: copy the file, apply the mutation, run the covering tests under a timeout (below), restore the file from the copy, and confirm `git status` is unchanged.
- If you are read-only, list the mutations, the expected red test and the targeted command. The lead runs them.

Run only the tests that cover the mutated file or module, never the whole suite: the single-test command from `docs/agents/stack.md` narrowed to them (`cargo test <module>`, `pytest <test file>`, one twister suite). Wrap each run in a per-mutation timeout, `timeout <seconds> <command>`: three times what the narrowest `stack.md` row covering that command lists, at least 60 seconds, or 300 seconds if no row lists a time. The lead builds each command from the `stack.md` single-test form, narrowed to an existing test path or module and never to free-form reviewer text, and applies mutations only inside the worktree, to files named in the diff. When no test covers the file, the mutation survives.

Record each result as one of:

- `killed`: the targeted run went red before the timeout.
- `survived`: the targeted run passed. This is an Important finding: name the missing test case.
- `inconclusive`: the run timed out (`timeout` exits 124), or it can't run on the host. Never report a timeout as killed. The lead reruns an inconclusive mutation once with double the timeout. If it is still inconclusive, record a Suggestion naming the condition left unproven.

```python
# diff adds:  if retries >= MAX_RETRIES: raise TimeoutError
# mutation:   if retries >  MAX_RETRIES: raise TimeoutError
# expected red: tests/test_uart.py::test_gives_up_after_max_retries
# targeted:     timeout 60 pytest -q tests/test_uart.py
```

Firmware: run mutations against the host-side unit tests or the simulator, never by flashing a board. If a branch is only reachable on target, the mutation is `inconclusive`.
