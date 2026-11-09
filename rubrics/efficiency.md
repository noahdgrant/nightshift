---
name: efficiency
pass: 0.6
---

Judge whether the unit's sessions worked towards the result or wasted effort.

Look for:

- failed tool calls that a look at the code or the instructions first would have avoided;
- steps repeated with no new information;
- exploration an instruction should have saved: a session searching for what a skill, a page under `docs/` or the unit's description could have said;
- workarounds for a wrong or missing instruction, including every one a session listed under `## Workarounds` in its summary.

Do not count work the unit needed: a failing test written first, a fix after a review finding, or a retry the repository's checks forced.

Scale:

- 1.0: every step moved the unit towards the result.
- 0.8: a little waste, such as one failed call or one repeated step.
- 0.6: noticeable waste, but the sessions kept their direction.
- 0.4: much of the effort went on failed calls, repeats, exploration or workarounds.
- 0.2: most of the effort was wasted.
- 0.0: the sessions never worked towards the result.
