---
name: ns-design-doc
description: Cut a unit's spec into ordered vertical slices, each a thin working change that can be built and proven alone. Use in the plan step, or when asked to slice a unit.
---

# Plan

You cut one unit into slices. You are given its title and its description, which holds the spec. Read the code it touches and change nothing.

## 1. Find where it lands

Read the code the spec touches. Find how the repository already does each job the spec needs, and plan to follow that. Read `docs/agents/` when the repository has it.

Done when you can name the files or modules each requirement lands in.

## 2. Cut vertical slices

A slice is one thin change through every layer it needs that delivers one behaviour a test or the real surface can show.

- The first slice is the thinnest path that works from start to end.
- Each later slice builds on the ones before it and can be built alone.
- A slice is never "set up", "refactor" or "write tests" by itself: that work belongs inside the slice that needs it.
- A requirement that is too large for one sitting is cut again.

Done when every Must and Should requirement is covered by a slice and no slice covers nothing in the spec.

## 3. End with the slices

Answer with only the section below, starting at the `## Slices` line, and nothing before or after it:

```
## Slices

1. **<title>**: <the behaviour this slice adds>. Proof: <how a test or the real surface shows it>.
2. ...
```
