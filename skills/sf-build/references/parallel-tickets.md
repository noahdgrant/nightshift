# Parallel tickets (planned)

Adapted from mattpocock `implement-spec`. Kept here for when `sf-plan` produces tickets with blocking edges. Until then, `sf-build` builds one unit per run.

The tickets are a **task graph**, not a list. At any moment a **frontier** of unblocked tickets is ready.

1. Read the spec and tickets and draw the graph.
2. Optionally hand exploration to a **fresh-context agent** that saves its notes outside the repo, where every implementer can read them.
3. Create one **integration branch**.
4. Hand each frontier ticket to its own **fresh-context agent**, in its own worktree (`sf worktree new <unit-id> --base <integration-branch>`), running `sf-build` on that ticket. Before reporting done, it merges the integration branch tip into its own branch.
5. Merge each finished ticket into the integration branch. If that unblocks tickets, start agents on them.
6. When every ticket is merged, run the next phases on the integration branch.
7. Remove the ticket worktrees with `sf worktree remove <unit-id>`.

Talk to the agents through **context pointers** (paths to the spec, tickets, notes and commits), not by restating their contents.
