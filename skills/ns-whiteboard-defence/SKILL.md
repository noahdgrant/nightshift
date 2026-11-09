---
name: ns-whiteboard-defence
description: Answer why something was implemented the way it was, or how something works, from the record and not from a guess. Use when the user asks why something was built that way, or how something works.
---

# Whiteboard defence

Reference and read-only: change nothing, and write no file. Answer in the shape of a whiteboard defence: how it works, why this and not that, what a hostile input does, where it fails. Use the words of `docs/GLOSSARY.md`, and the word "change request", never a forge's own word for it.

## 1. Find the record

Follow the code the question is about to the commit that made it: `git log`, `git blame`, and `git log -S '<text>'` for code that has moved or gone. Then follow the commit to the change request and the unit it names, and read:

- the unit's description and its discussion;
- the change request's body: the build summary and its `## Defend this` section;
- the kept sessions of the unit (`ns log <unit>`), when they are still there;
- `docs/GLOSSARY.md` and the pages under `docs/`.

A commit that names no unit, a unit that is gone, or a session that was not kept is a gap in the record. Say so, and go on with what is left.

Done when each claim the answer will make has a source, or is marked as inferred.

## 2. Answer

Lead with the direct answer, in a few sentences. Then add only the parts of the defence the question touches:

- **Why this and not that**: what the record says about the alternative.
- **Hostile or malformed input**: what such an input does at each boundary the code touches.
- **Where it fails**: its limits, how it fails and what it does not handle.

Mark each reason `recorded`, with where it was recorded (the commit, the change request, the unit, a session or a page), or `inferred`. When nothing says why, write "the record does not say", give what the record does show, and invent no reason. An inferred reason is a guess and is labelled as one.

End with the one question about this that a reviewer would most likely ask next, and its answer, or say that it is open.
