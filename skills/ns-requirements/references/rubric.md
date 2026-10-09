# Review rubric

Use this on your own draft before showing the author, and on someone else's doc when asked to review. Score each line pass or fail. A fail is a specific thing to fix.

## Blocking

- [ ] The header states the kind, the version, and the scope.
- [ ] Every requirement is pass/fail: a measure, a threshold, and the conditions. No requirement is a direction ("improve", "robust", "easy").
- [ ] Every requirement has an ID, and each row holds one check.
- [ ] Requirements name outcomes, not mechanisms, unless the mechanism itself is required and sourced.
- [ ] Every number gives its source.
- [ ] A Why / source that rests on another of the project's docs cites its ID or section instead of repeating its figures.
- [ ] Every `[?]` has an Open questions row, and every Open questions row has an owner by name and a Needed by milestone.
- [ ] No Must exists only to serve a Should or a Could. No Should or Could is a precondition of a Must.
- [ ] Every Won't gives a reason.
- [ ] Standalone and system docs: every requirement cites a story or a stated need. Component docs: every requirement cites a parent ID, a system story, or a sourced constraint.
- [ ] System docs: every row names who it is allocated to, and each allocated component has a doc or an owner.
- [ ] Component docs: no row outranks its parent, and no row repeats one from the system doc.
- [ ] Component docs: every system row allocated to the component is either cited by a row or named on the "Met as written" line. A row that is neither is usually a gap.
- [ ] Status is not `Approved` unless a person approved it.
- [ ] No `>` prompt lines remain.

## Quality

- [ ] The stories cover every actor who touches the thing, not only the buyer.
- [ ] Every story is served by a requirement or named as a Won't.
- [ ] Every Must says how it is verified.
- [ ] When most items are Musts, the author has said what gets cut first. A list that is all Musts hasn't been prioritized.
- [ ] Terms match the repo's `GLOSSARY.md` when one exists.

## Smells, with the fix

The examples are an illustrative street parking system (meters, a payment server, a driver app), not real decisions.

| Smell | Weak | Strong |
|---|---|---|
| A capability with no bar | "Detect meter faults" | "Report a jammed coin slot to the server within 5 min" and "Report a dead display within [?] min", as two rows |
| A mechanism as a requirement | "The ability to use a cellular modem to report payments" | "Report each payment to the server within [?] s of the driver paying". The modem goes in the design doc |
| A number with no parent | "Sleep current less than 2 mA" | A requirement for the need, "A meter runs [?] months between battery swaps", and the current cites it: "Sleep current < 2 mA. Serves the battery-swap requirement, from a [?] Ah pack" |
| An adjective as the bar | "The cash box is hard to break into" | "Opening the cash box takes longer than [?] minutes without the service key" |
| A placeholder hidden in the prose | "Sync the tariff every X hours" | "Sync the tariff at least every [?] hours", with an Open questions row |
| Contradictory priority | Card payment is a Must, the receipt it depends on is a Should | Make the receipt row trace to the card row and share its priority, or give it a parent of its own |
| A promise no component owns | System: "the driver is warned before their time runs out". Meter doc: reports the expiry to the server. App doc: nothing about warnings | Allocate the system row to every component on the path, and give each a row that refines it |
| A Won't with no reason | "Licence plate recognition" | "No licence plate recognition. Storing plates needs a privacy review the pilot can't wait for. Revisit after [?]" |

## How to deliver a review

Lead with the two or three problems that would cost the most during design or acceptance, and say what each would cost. Then list the rest. Rewrite a few representative rows so the author sees the bar, and say plainly whether the doc is ready to design against.
