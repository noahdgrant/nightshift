---
name: redundant-tests
pass: 0.6
---

Judge whether the unit's tests each prove something no other test already proves, at the cheapest kind of test that can prove it. Read the repository's testing guidance first (`docs/CODING_STANDARDS.md`, `.nightshift/coding-standards.md`) and judge against it.

Look for:

- tests that repeat what an existing test already proves, with only different inputs that take no new branch;
- tests that check through a slow kind of test (a whole command run, a harness session) what a unit test of one function proves;
- several tests that fail together for the same one cause;
- tests that assert what the compiler or a library already guarantees.

Do not count a test for a branch or error path nothing else takes, or a slower test that proves how parts work together.

Scale:

- 1.0: every test proves something new, at the cheapest kind of test that can.
- 0.8: one test repeats another or sits at a dearer kind of test than it needs.
- 0.6: a few tests are redundant, but most earn their place.
- 0.4: many tests repeat each other or sit at a dearer kind of test than they need.
- 0.2: most of the tests are redundant.
- 0.0: the tests prove nothing the others do not, or there are only redundant ones.
