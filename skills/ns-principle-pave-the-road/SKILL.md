---
name: ns-principle-pave-the-road
description: "Apply when adding a pattern, helper, dependency or tool, or when a second way to do one job appears. Agents take the shortest path and copy what's nearby, so keep one paved road per job and make it the shortest path."
disable-model-invocation: true
---

# Pave the Road

Agents take the shortest path to done, and they copy the code around them. Whatever the repo makes easiest, and whatever it already does, is what gets written next. So the shortest path must be the correct one: one **paved road** per job.

This covers code and config. For docs and skills, the `ns-writing-for-agents` skill keeps each meaning in one place.

**1. One road per job.** Each job (fetch config, raise an error, talk to the board, parse a frame) has one way it's done, defined in one place. A second way is a fork in the road: the next agent copies whichever it saw last, and the copies drift. When you find two, [migrate the callers to one and delete the other](../ns-principle-migrate-callers-then-delete-legacy-apis/SKILL.md).

**2. Make the road the shortcut.** If the right way is longer than a wrong way, agents take the wrong one. Shorten the right way (a helper, a default, a generated stub) or block the wrong one, as high up this ladder as you can: architecture that makes it unwritable, then lint, compiler and CI, then skills and rules, then review. [Encode lessons in structure](../ns-principle-encode-lessons-in-structure/SKILL.md) says how.

**3. Pave with well-trodden stones.** Prefer the conventional tool, library and pattern: the repo's existing choice first, then the ecosystem's standard. Agents do best on ground they've seen most. When the existing choice is the problem, file a migration; never fork the road inside a feature change. A novel choice needs a stated reason, and once chosen it becomes the road: migrate to it rather than adding a third way.

**Anti-patterns:**
- A near-duplicate helper "because the old one was awkward"
- A new dependency where the standard library or an existing dependency covers it
- The old way left alive "for compatibility" with no external consumer
- A recurring agent mistake fixed with a comment instead of a shorter right path
