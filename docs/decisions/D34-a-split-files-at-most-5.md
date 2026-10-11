---
id: D34
title: "A split files at most 5 children"
issue: 133
date: 2026-10-10
amends: [D26]
---

# D34: A split files at most 5 children

## Decision

Amends D26. A split files at most 5 children, existing ones included; past that it files none, posts Triage Notes saying why, and moves the parent to `needs-define` (`needs-info` if the reporter must answer). A parent with an outside origin (author not confirmed team, or an `Origin:` line) gets children that stay `needs-triage` under any gate and carry `Origin: <login> (outside the team or unconfirmed)`. Each child is added to the parent's Triage Notes as soon as it is filed. A rerun reuses every existing child, found through the notes or a tracker search, that is team-authored and links the parent, and files only the missing ones. `Blocked by` names only a sibling or an issue verified on the tracker.

## Reason

D26 put no bound on a split and sent every child to `ready-for-agent` under `gates: auto`, so an outside reporter could steer the factory into filing and building any number of issues, and a run killed midway filed duplicates on rerun (#133). Five children is what a human can check in one sitting. An outside reporter's request gets a team member's look before building, the line D31 draws. The outside origin goes on its own `Origin:` line, not `Reported by:`, because local-markdown trackers read `Reported by:` as the author: a child marked that way would look outsider-authored and a rerun would refile it. Triage Notes only nominate children; each one is checked on the tracker, so forged notes can't stand in for a real child. Known limit: the outside-origin guarantee rests on the skill reading that body line; the CLI does not know it. Cases `over-cap-split-asks-human` and `external-author-split-needs-triage` cover the cap and the outside rule; rerun safety has no case yet (the brief asked for these two; a local-markdown fixture with a parent whose notes list one filed child could pin it), and the blocker source needs a tracker lookup. Spec: skills/ns-triage/SKILL.md.
