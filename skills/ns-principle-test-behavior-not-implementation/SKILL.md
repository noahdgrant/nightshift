---
name: ns-principle-test-behavior-not-implementation
description: "Apply when you write, change, or keep a test. Call the code the way its users do and assert the result they observe against a literal expected value. If the test would still pass when every imported function returns None, rewrite the assertion or delete the test."
disable-model-invocation: true
metadata:
  upstream: cursor/plugins@ccb5507cec15:pstack/skills/principle-test-behavior-not-implementation
---

# Test Behavior, Not Implementation

A test calls the code the way its users do and asserts the result they observe against a literal expected value. A test that asserts which calls the code made, or restates a constant the code contains, does neither.

The check: before you keep a test, ask whether it would still pass if every function it imports returned `None`. If yes, it observes no behavior and cannot fail for a defect. Rewrite the assertion or delete the test.

**Why:** A test that cannot fail for a defect costs CI time and review attention and catches nothing. A constant pin also fails when someone edits the constant or the prompt it restates, so it prevents that edit.

**Five shapes that still pass when every imported function returns `None`:**

- **Weak or no assertion.** No `assert`, or only `assert result`, `is not None`, `isinstance(...)`, `> 0`, or a call that only shows nothing raised.
- **Mock or absence only.** Only `mock.assert_called()`, `assert_not_called()`, `is None`, `== []`, `len(x) == 0`, `!= wrong_value`.
- **Self-referential.** The expected value comes from the code under test: `assert f(a) == f(a)`, `assert parsed.url == build_url(...)`.
- **Constant pin.** The assertion restates a hand-maintained constant, config default, table row, or prompt string: `assert LIMITS.max_tools == 8`, `assert "You are" in PROMPT`.
- **Fixture asserts fixture.** The assertion reads data the test built or a value a pytest fixture computed, and the subject never runs inside the body.

**The fix:** call the subject inside the test body with one concrete input and assert the literal output or the observable effect, `assert slugify("Hello, World!") == "hello-world"`. For an absence, assert the presence on the other input in the same test. For a constant, test the mechanism that reads it with one input instead of restating the value. For a mock, assert the payload it received or the state after the call, not that it was called. When no such assertion exists, delete the test.

**Firmware:** with a fake HAL or register map, assert the value written to the register or the bytes put on the bus, not that the write function was called.

**Keep** a test of a relation across a table's rows (a key present in two tables, a parent that exists), and a static check the type checker runs (`typing.assert_type`).
