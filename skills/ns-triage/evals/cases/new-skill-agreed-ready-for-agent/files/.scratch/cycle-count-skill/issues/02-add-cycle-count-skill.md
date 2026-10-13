# Add a cycle-count agent skill

Status: needs-triage
Category:
Priority: priority:medium

Reported by: Sam (purchasing)

Design: 01 (closed), the agreed design for this skill.

## Description

Once a month someone counts the shelves and compares the counts with `inventory status`. I'd like an agent skill for it, next to `skills/receive-delivery/`, so an agent can walk through the comparison and tell us which items are off.

## Acceptance criteria

- [ ] Add `skills/cycle-count/SKILL.md` with `name: cycle-count` and a description that says when to use it.
- [ ] The skill reads a counts file, one `<sku> <counted>` per line.
- [ ] For each SKU it compares the count with on-hand stock from `inventory status <sku>` and reports every SKU whose count differs, with both figures.
- [ ] A SKU missing from the counts file is reported as not counted.

## Comments
