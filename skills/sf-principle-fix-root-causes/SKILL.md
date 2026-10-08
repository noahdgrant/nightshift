---
name: sf-principle-fix-root-causes
description: "Apply when debugging. Trace each symptom to its root cause and fix it there; reproduce first, ask why until you reach it, resist nil-check guards that silence crashes."
disable-model-invocation: true
metadata:
  upstream: cursor/plugins@ccb5507cec15:pstack/skills/principle-fix-root-causes
---

# Fix Root Causes

When debugging, fix the root cause, not the symptom. Trace every problem to its root cause and fix it there.

**Why:** Symptom fixes accumulate. Each workaround makes the system harder to reason about, and the real bug remains. Root-cause fixes are slower upfront but reduce total debugging time.

**Pattern:**
- Reproduce first, ideally as a failing test
- Ask "why" until you hit the root cause
- Leave out guards that only hide the failure (an `if x is None: return` that silences a crash, or a bare `except`, is a symptom fix)
- If a workaround needs a paragraph-long comment to justify it, the code is wrong (fix the code, not the comment)
- Check for the pattern, not just the instance (grep for the same pattern, fix all instances)
- When stuck, instrument instead of guessing (add logging, read the actual error and traceback)

**Restart bugs: suspect state before code**

When something "fails after restart," suspect stale persistent state first: config files, caches, lock files, serialized state. On firmware, that includes values kept in flash, EEPROM, or retained RAM across a reset. If clearing that state restores behavior, prioritize state validation as the fix.
