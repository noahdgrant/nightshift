---
name: sf-principle-experience-first
description: "Apply when product, UX, or feature-scope tradeoffs come up. Choose user delight over implementation convenience; ship fewer polished features over more rough ones."
disable-model-invocation: true
metadata:
  upstream: cursor/plugins@ccb5507cec15:pstack/skills/principle-experience-first
---

# Experience First

When implementation convenience conflicts with user delight, choose delight.

- Every feature, control, and option must be justified
- Ship less, ship better (polished experience with three features beats rough one with ten)
- Prototype before committing (design decisions are cheaper in a throwaway prototype than in production code)
- Get the details right (transitions, alignment, spacing, feedback, error states and messages)
- Tighten the core loop (every feature should serve the central workflow or get out of the way)

The user is whoever consumes the work. For a UI that is the end user. For a library, a CLI, or an internal API it is the colleague who calls it. The engineer who maintains the code next is a user too. Weigh their experience the same way, and explain impact from their perspective.

Foundations should serve the experience. [Foundational Thinking](../sf-principle-foundational-thinking/SKILL.md) governs the *sequence* of work. This principle governs the *target*.
