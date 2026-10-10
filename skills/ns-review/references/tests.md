# Tests reviewer

What must hold on this axis is the **Tests** section of the [quality bar](../../ns-contract/references/quality-bar.md). This file is how to find where it doesn't.

Do the tests prove the change works, and would they catch it breaking? Read the tests before the implementation: they reveal intent and coverage.

## Look for

- **Existence**: every new behaviour and every bug fix has a test. A bug fix with no test that fails before the fix is an Important finding.
- **Behaviour, not implementation**: tests go through the public interface and survive a refactor. Tests asserting on private helpers, call counts or internal state are findings. See [test behaviour, not implementation](../../ns-principle-test-behavior-not-implementation/SKILL.md).
- **Right level**: pure logic gets a unit test; crossing a boundary gets an integration test; a critical user flow gets an end-to-end test. Test at the lowest level that captures the behaviour.
- **Scenarios**: happy path, empty input, boundaries (min, max, zero, negative), error paths (invalid input, timeout, I/O failure), concurrency (repeated or out-of-order calls).
- **Mocks at system boundaries** (network, database, hardware), not between internal functions.
- **Independence**: no shared mutable state or ordering between tests. Each test checks one concept.
- **Names** read like a specification: `test_gives_up_after_max_retries`, not `test_retry_2`.
- **Snapshot tests** nobody reviews, and tests that can never fail. A test that never fails is as useless as one that always fails.
- **Checks on the real thing**, not a proxy: the actual output artifact, not a log line or a self-report.

```python
def test_rejects_frame_longer_than_max_payload():
    frame = header(length=MAX_PAYLOAD + 1) + bytes(MAX_PAYLOAD + 1)
    with pytest.raises(FrameError):
        parse_frame(frame)
```

Firmware: logic that can run on the host should have host unit tests. Hardware-dependent behaviour should sit behind a seam with a fake, plus a simulator or hardware-in-the-loop test where `docs/agents/stack.md` lists one. A change only checked on target, with no recorded run, is `inconclusive`.

## Report

For each gap, name the missing test: what it verifies and why it matters. Severity: a missing test for a behaviour, branch or error path the diff adds is Important. In code the diff didn't touch, a missing test for data loss, security or an acceptance criterion is Important; other gaps are Suggestions.
