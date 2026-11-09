---
name: followed-the-brief
pass: 0.6
---

Judge whether the unit did what its description asked and nothing beyond it, and kept the repository's coding standards (`.nightshift/coding-standards.md`, `docs/CODING_STANDARDS.md`).

Look for:

- a requirement in the description that the change does not do, or does only in part;
- work the description did not ask for: extra options, refactors, files or behaviour outside its scope, or something it lists as out of scope;
- a rule of the coding standards that the change breaks.

Do not count a change the checks forced, a fix after a review finding, or an assumption the build summary named for something the description left open.

Scale:

- 1.0: every requirement is done, nothing beyond them was, and the standards were kept.
- 0.8: one small slip, such as a minor extra or a single standards rule broken.
- 0.6: a requirement is only partly done, or a few extras or slips, but the unit's goal is met.
- 0.4: a requirement is missing, or much work went beyond the brief.
- 0.2: most of the requirements are not done.
- 0.0: the change does not do what the description asked.
