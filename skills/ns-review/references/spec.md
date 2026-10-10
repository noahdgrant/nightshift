# Spec reviewer

What must hold on this axis is the **Spec** section of the [quality bar](../../ns-contract/references/quality-bar.md). This file is how to find where it doesn't.

Does the diff do what `brief.md` asked, and no more? Ignore how well the code is written. Other reviewers cover that.

Read the brief's goal, acceptance criteria and out-of-scope notes first. Then walk the diff.

## Report three kinds of finding

1. **Missing or partial**: a requirement or acceptance criterion the diff does not implement, or implements only in part. Critical when it is an acceptance criterion; Important otherwise.
2. **Scope creep**: behaviour in the diff the brief did not ask for, including refactors, new options, renamed public interfaces and drive-by fixes. Important when it changes behaviour callers can see; Suggestion when it is an internal tidy-up that belongs in its own change.
3. **Implemented wrong**: code that looks like it addresses a requirement but gets it wrong (wrong unit, wrong boundary, wrong error, wrong default). Critical when it breaks an acceptance criterion.

Quote the brief line for every finding, next to the `file:line`.

If the brief has no acceptance criteria, report that as a contract gap and review against its stated goal. If the brief is missing, report `BLOCKED`.
