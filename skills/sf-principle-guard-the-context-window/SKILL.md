---
name: sf-principle-guard-the-context-window
description: "Apply when context is filling up: large outputs, long files, repeated reads, fan-out planning. Route bulk to fresh-context agents; keep summaries in the main thread, not raw payloads."
disable-model-invocation: true
metadata:
  upstream: cursor/plugins@ccb5507cec15:pstack/skills/principle-guard-the-context-window
---

# Guard the Context Window

The context window is finite and non-renewable within a session. Every token should be worth its cost.

**Why:** Context overflow degrades reasoning quality, creates compression artifacts, and halts progress.

**Pattern:**
- **Isolate large payloads.** Hand verbose outputs, logs, captures, screenshots, and large documents to a **fresh-context agent**. The main context gets summaries, not raw data.
- **Keep frequently used content inline.** Templates and references used on every invocation belong in the skill file, not in separate files that cost a read each time.
- **Size phases and cap scope.** Limit files per phase, set turn budgets, account for mechanism costs.
