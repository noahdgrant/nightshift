# The design tree for a design doc

The `ns-grilling` skill runs the interview, and the `ns-domain-modeling` skill keeps the glossary and ADRs up to date as you go. This file adds which branches the tree must cover and when a branch counts as settled.

## Branches

Problem before solution, always, even when the author arrives with the solution. Most do, and the problem statement is the section that gets skipped and then argued about in review. The list is in prerequisite order.

1. **Scope and kind.** What are you changing, and what breaks if you get it wrong? If nothing is changing and the author wants an existing component written down, it is an as-built doc.
2. **Problem.** What is happening today? Ask for one concrete instance: a ticket, a log, a rate, a customer sentence. Then ask why now. For an as-built doc, ask what the component exists to solve instead. When a requirements doc exists, ask which of its IDs this design answers.
3. **Constraints.** What can't move? Chase the source of each one, because a constraint no one can attribute is often a habit.
4. **The design.** Let them talk it through, then read it back as an ordered walkthrough and ask where you got it wrong.
5. **Boundaries.** Which contracts does this touch? What does an old peer do when it meets a new one? Ask this even when they say nothing changes.
6. **Alternatives.** What else did you consider, and what did anyone else want to do? Ask directly why do-nothing loses. For an as-built doc, ask which decisions were argued and which were assumed.
7. **Failure.** Go part by part: what happens if this fails, and how do you find out? Then offer the boring ones from the template's Failure modes prompt and see which land.
8. **Privacy and Security.** What data does the path carry, and does anything new get a way in? Both sections are in every doc, so both need an answer, even a one-line one.
9. **Validation and rollout.** How do you know before the field knows? What is the back-out? For an as-built doc, ask what coverage exists and what is missing.
10. **Non-goals.** Ask last, when the shape is clear. "What are people going to ask you to add to this that you want to refuse?"

## When a branch is settled

Push back once on a vague answer. "It'll handle retries" gets "how many, how far apart, and what happens when they run out?" If the author still cannot answer, record it as an open question with an owner and a Needed by milestone. That settles the branch for the interview. The doc goes out with the gap named rather than waiting on it.

## Questions that pull real answers

- "What would make you revert this?"
- "Who is going to disagree with this doc, and where?"
- "What did you try that didn't work?"
- "If this ships and it's wrong, how do we find out? Who finds out first, us or the customer?"
- "Which part are you least sure about?"

The last one is the most useful question in the set. The answer is usually the real content of the doc.

## Questions to skip

- "What sections do you want?" The template decides that.
- Anything about formatting. Nobody has an opinion until they see the draft.
