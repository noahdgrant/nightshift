# Spec reviewer

What must hold on this axis is the **Spec** section of the [quality bar](../../ns-contract/references/quality-bar.md). This file is how to find where it doesn't.

Does the diff do what `brief.md` asked, and no more? Ignore how well the code is written. Other reviewers cover that.

Read the brief's goal, acceptance criteria, instances, failure modes, edge cases and out-of-scope notes first. Then walk the diff, and for each instance, failure mode and edge case the brief lists, find the code that handles it.

## Report three kinds of finding

1. **Missing or partial**: a requirement or acceptance criterion the diff does not implement, or implements only in part, including a failure mode or edge case from the brief that the diff leaves unhandled. Critical when it is an acceptance criterion; Important otherwise. For a brief with **Instances**, this includes a **partial fix**: an instance the brief covers that the diff leaves unfixed, or a diff that adds no guard for the class (a rule, test or type that stops the defect coming back). A partial fix is Important at least. An instance the brief marks filed separately is out of this unit: leave it.
2. **Scope creep**: behaviour in the diff the brief did not ask for, including refactors, new options, renamed public interfaces and drive-by fixes. Important when it changes behaviour callers can see; Suggestion when it is an internal tidy-up that belongs in its own change.
3. **Implemented wrong**: code that looks like it addresses a requirement but gets it wrong (wrong unit, wrong boundary, wrong error, wrong default), including a failure mode or edge case handled differently from what the brief says. Critical when it breaks an acceptance criterion.

Quote the brief line for every finding, next to the `file:line`.

Behaviour for a failure or edge case the brief does not list stays scope creep (kind 2).

A PR-text criterion in an older brief (the body, `Closes`, labels, merge) and the brief's Ship note belong to `ns-ship` ([ship note](../../ns-triage/references/agent-brief.md#ship-note-pr-text)): never report them as a missing acceptance criterion.

If the brief has no acceptance criteria, report that as a contract gap and review against its stated goal. A brief written before Failure modes, Edge cases and Instances were required may lack them: report the missing section as a contract gap, never as a finding against the code. If the brief is missing, report `BLOCKED`.
