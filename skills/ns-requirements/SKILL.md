---
name: ns-requirements
description: Turn a vague unit into a spec a builder can work from, and write it to the file you are given. Use in the define step, once the user has been grilled, or when asked to write the requirements of a unit.
---

# Requirements

You turn one vague unit into a spec. You are given its title and description, and the path of the file to write. The user is in this session with you.

## 1. Settle what is asked

Grill the user first, with the grilling instructions above, until the goal, the edges and what is out of scope are settled. Read the code the unit touches before you ask about it, and ask only what the code cannot tell you.

Done when nothing the spec needs is left silently assumed.

## 2. Write the spec

Use these sections, in this order:

1. **Goal**: one or two sentences on the outcome a person gets, never the mechanism.
2. **Requirements**: grouped as Must, Should, Could and Won't. Each line is something a tester can pass or fail. Where a person uses the thing, add a story: "As a …, I want …, so that …".
3. **Out of scope**: what the user ruled out, so the builder does not build it.
4. **Open questions**: what only the user can answer. Leave none open; if one remains, ask it now.

Say what is true when the unit is done, never how to build it. Choosing the design is the plan's job.

Done when every requirement can be passed or failed and nothing in it names a mechanism.

## 3. Confirm, then write

Show the user the spec. Write it to the file you were given only after they say it is right, then stop. Never write a file anywhere else.
