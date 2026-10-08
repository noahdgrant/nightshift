---
name: sf-principle-redesign-from-first-principles
description: "Apply when integrating a new requirement into an existing design. Redesign as if the requirement had been a foundational assumption from day one, instead of bolting it on."
disable-model-invocation: true
metadata:
  upstream: cursor/plugins@ccb5507cec15:pstack/skills/principle-redesign-from-first-principles
---

# Redesign From First Principles

When integrating a change, redesign as if the requirement had been there from the start instead of bolting it onto the existing design.

- Read all affected files and understand the current design
- Ask: "if we were writing this from scratch with this new requirement, what would we build?"
- Propagate the change through every reference: types, docs, examples, rationale sections
- Think about the whole redesign, then deliver it incrementally

This is the method for preserving option value when integrating changes into an existing design.
