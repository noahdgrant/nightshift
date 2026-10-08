---
name: sf-principle-never-block-on-the-human
description: "Apply when tempted to ask 'should I do X?' on reversible work. Proceed, present the result, let the human course-correct after the fact; reserve confirmation for irreversible actions."
disable-model-invocation: true
metadata:
  upstream: cursor/plugins@ccb5507cec15:pstack/skills/principle-never-block-on-the-human
---

# Never Block on the Human

The human supervises asynchronously. Agents must stay unblocked. Make reasonable decisions, proceed, and let the human course-correct after the fact.

**Why:** Every permission pause stalls the pipeline and makes the human the bottleneck. Since code changes are reversible and reviewable, a wrong decision usually costs less than blocking.

**Pattern:**
- **Proceed, then present.** Do the work, show the result. Instead of asking "should I do X?", do X and explain why.
- **Answer facts by running something.** If an experiment can settle the question (behavior, timing, output), run it instead of asking.
- **Make the system self-healing.** When you notice a problem, log it and fix it in the next round.

**Boundaries:**
- **Irreversible actions** still require confirmation: force-push to a shared branch, merging, deploying or releasing (including OTA and flashing production units), deleting data, and messaging anyone outside the team.
- **Reversible actions** (write code, edit notes, split tasks) proceed without blocking.
- **Product direction** comes from the human. *Execution* should not block. When a call is the human's, apply a sensible default, record it in the artifact, and say what they could choose instead.
