# The design tree for a requirements doc

The `ns-grilling` skill runs the interview. This file adds which branches the tree must cover and when a branch counts as settled.

## Branches

In prerequisite order.

1. **Kind, scope, and key.** Look for existing requirement docs, issues, and design docs before asking; read any the thing belongs to. Then settle whether it is standalone, a system, or a component, and what the priorities are relative to. For a new project with no scope yet, start with the problem: what is broken or missing, for whom, and what the first usable version must do.
2. **Who and why.** Standalone or system: every person who touches the thing, in a role, and what each needs. Write these up as stories and read them back. Ask about the actors people forget: installer, support, the person who removes it. Component: the parent requirements and stories this component serves, and the constraints that bind it, each with a source. A need with no parent in the system doc goes into the system doc first.
3. **Requirements.** Derive them from the stories or parents, then take each through the testable push: what is measured, what threshold, under what conditions, and who checks it. Hunt for mechanisms posing as requirements and ask what outcome the mechanism was for.
4. **Priority.** Put each requirement in a category using the Must test in `SKILL.md`. Walk the Musts one at a time. Then look for contradictions: a Must that only exists to serve a Should or a Could, and in a component doc, a row that outranks its parent.
5. **Won't.** "What will people ask for that you want to refuse in this scope, and why?" Also sweep the stories: any story with no requirement is a Won't or a gap.
6. **Verification.** How would someone check each Must? A Must with no check usually isn't testable yet.

## When a branch is settled

Push back once on a vague answer. "Hard to remove" gets "by whom, with what tools, in how long?" If the author still can't answer, write `[?]` and add an Open questions row with an owner and a Needed by milestone. That settles the branch; the doc goes out with the gap named rather than waiting on it.

## Questions that pull real answers

- "If we had to ship next month, which of these would you cut first?"
- "Who would be angry if this item were missing, and what would they do?"
- "What is this number protecting?" Asked of every threshold, it usually uncovers the parent requirement.
- "What do you mean by done?"
