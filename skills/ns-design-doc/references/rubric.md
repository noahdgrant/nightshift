# Review rubric

Use this on your own draft before showing the author, and on someone else's doc when asked to review. Score each line pass or fail. A fail is a specific thing to fix, not a grade.

For an as-built doc, read "Alternatives" as the decisions already taken, skip the before-and-after and back-out checks, and skip the rollback trigger. `SKILL.md` has the details.

## Blocking

These make the doc unreviewable. Fix before circulating.

- [ ] The problem is stated with at least one checkable fact, and it is separable from the proposed solution.
- [ ] Goals are pass/fail. No goal is a direction ("improve", "optimize", "modernize").
- [ ] When a requirements doc exists, the header names it with the version this design answers, and each goal opens with a requirement ID.
- [ ] Every number, allocation, priority, or division of work that a requirement, another design, or an interface sets is cited by ID or linked section, and any gist written inline has the ID beside it.
- [ ] The design names real identifiers: object and resource numbers, topic names, message types, columns, config keys, commands. Not generic nouns.
- [ ] Every boundary the change touches is shown before and after, and the doc says what happens when an old peer meets a new one.
- [ ] The design, the state section, and the privacy and security sections describe the same change. Nothing appears in one that the others contradict or leave out.
- [ ] Alternatives includes do-nothing, plus any option a colleague actually argued for, each with a specific reason it lost.
- [ ] Every failure mode names how it is detected.
- [ ] The back-out is a concrete action. If there is none, the doc says so and names what stands in for it.
- [ ] Privacy and Security are both present. A one-line Privacy section names the data the path carries; a one-line Security section says no entry point, credential, or permission changes.
- [ ] If the change carries, stores, or exposes identity, location, contact, or customer data, Privacy names the fields, the retention, and the deletion path.
- [ ] If the change adds an entry point, a credential, or a permission, or exposes data to a new party, Security describes that change rather than the system's general posture.
- [ ] Open questions have named owners and a Needed by milestone each.
- [ ] Status is not `Approved` or `Superseded` unless a person set it.
- [ ] No `>` prompt lines remain, and no section reads "N/A".

## Quality

- [ ] Non-goals exist and would each survive a reviewer pushing on them.
- [ ] Constraints cite a source: datasheet, spec section, measurement, or person. None of them is a setting the team chose and could change.
- [ ] Numbers a reader would act on give their source: measured, estimated, datasheet, or config.
- [ ] Validation names a quantity and a rollback trigger.
- [ ] Rollout states the ordering between sides and whether each step deploys on its own.
- [ ] Any diagram replaces prose rather than repeating it, and its arrows carry real message names.
- [ ] Terms match the repo's `GLOSSARY.md` when one exists, and none of the words it lists under _Avoid_ appear.

## Smells, with the fix

| Smell | What it usually hides | Fix |
|---|---|---|
| "Handles", "manages", "supports" | The mechanism is not decided yet | Name the mechanism, or move it to Open questions |
| "Robust", "scalable", "efficient" | No measurement exists | State the number, or the test that would produce it |
| A section reading "N/A" | The template was obeyed rather than used | Delete it if it is a triggered section. If it is always included, say in one line why there is nothing to add |
| Alternatives that are all obviously bad | Written after the decision, to justify it | Add the option someone actually wanted, fairly stated |
| No failure section, or one paragraph of reassurance | The edges have not been walked | Go part by part and build the table |
| Length above five pages for a proposal, or eight for an as-built doc | Reference material is inline | Move it to an appendix or a linked doc |
| Every heading filled, nothing surprising in any of them | The doc was produced, not thought through | Ask the author which part they are least sure about, and write that |
| "Compliant", "no new personal data" with no field list | Nobody checked what the path carries | List the fields, then say which are personal and why |
| A security section listing SQL injection, XSS, and DDoS | Generic text written without looking at the change | Describe the one new capability and what bounds it |
| Technology stack or infrastructure recited in full | Static facts copied from the last doc | Cut it, or keep only what the change adds |
| A per-component breakdown with five thin subsections each | The structure was imposed rather than needed | Collapse to prose for components with little to say |

## How to deliver a review

Lead with the two or three gaps that would cost the most to find during implementation, and say what it would cost. Then the smaller things as a list. Skip line edits unless they asked.

Say plainly whether you would approve it. A review that lists concerns but never takes a position leaves the author guessing, which is how docs sit in review for three weeks. Your view is advice: approving the doc stays with a person.
