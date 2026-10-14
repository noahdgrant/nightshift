# Unit lifecycle

A unit is one issue's work, from the issue to its merged change. It has the issue, a worktree `<repo>.worktrees/<unit>` on the branch `ns/<unit>`, its artifacts in the worktree's `.ns/<unit>/`, a PR, events in the run log, and quality records.

1. **Queued.** The issue gets `ready_label`: from a person, or, for `ns watch`'s triage pass, from `ns watch` itself, reading the outcome the triage-only run wrote to `brief.md` (see ["Status labels"](watch.md#status-labels)). That run has already created the worktree and branch and written `brief.md` there.
2. **Started.** `ns watch` swaps the label for `in_progress_label` and runs `ns run --issue <n>`. That creates the worktree from `origin/<default>`, or reuses the one an earlier run left, and runs `[worktree] setup`.
3. **Phases.** Triage, build, verify, review and ship run in the worktree, each writing its artifact to `.ns/<unit>/`. Ship pushes `ns/<unit>` and opens the PR, whose body closes the issue.
4. **Ended.** The unit ends `merged` (the merge step merged it), `done` (a human merges), `stuck`, `split`, `paused` or `budget`. A `split` unit's issue waits under `split_label` until its children are done, and `ns watch` closes it then (see ["Split parents"](watch.md#split-parents)). On `merged`, `done` and `stuck`, `ns run` writes the unit's quality records.
5. **Cleaned up.** Once the change merges and the records are saved, the worktree, its artifacts and the local branch are removed. `ns run` does it right after its own merge. `ns watch` does it at start and between units for a PR a human merged. `ns clean` does it on demand. The merge step deletes the remote branch after its merge, and for a human merge the repo's setting decides. Nothing a unit runs checks out the default branch in its worktree.

After cleanup, the run log and transcripts under the git common dir, the quality records on `nightshift/quality`, the PR and the issue remain. A unit whose issue is still open keeps its worktree, so the next run resumes it: a stuck unit, a paused or interrupted one, parked work, or a triage-only run that ended needs-info or needs-define.

## Cleanup

```
ns clean [--dry-run]
```

A unit's worktree goes when its change merged. For `ns run`, that is its own merge step reporting `MERGED`. For `ns watch` and `ns clean`, all of these must hold:

- The unit id starts with an issue number (`142-uart-timeout`), and that issue is closed.
- `pr.md` (or its newest archived copy) names a PR, and `gh pr view` reports it `MERGED`.
- The PR's body closes the issue (`Closes #n`, or fixes or resolves), and its head branch is `ns/<unit>`.

Its quality records are saved first, with outcome `merged`, all of the sweep's units in one push. A record that lands in the outbox counts as saved: the outbox keeps it until the next push. A record that can't be saved at all keeps the worktree, and so does a review artifact that can't be read (no frontmatter, say), since no record holds it.

Cleanup never removes:

- A worktree with uncommitted changes to tracked files. Untracked and ignored files, such as build output and `.ns/`, go with the worktree.
- A worktree whose HEAD is not on `ns/<unit>`: detached, or on another branch. The one exception is a worktree on the default branch at `<repo>.worktrees/<unit>`, as an older merge step left it with `gh pr merge --delete-branch`: if its unit merged, it goes too. Its commits are on the default branch, so no branch-holds check applies, and no branch is deleted.
- A worktree whose branch holds changes the merged PR's head doesn't. The head holds the branch when it is the branch's tip, descends from it (`update-branch` merged the base in), or carries the same change against the default branch, whitespace included (`git patch-id --verbatim`, as after a rebase). Currency ignores whitespace; cleanup doesn't, because a re-indented line can change what code does. A head missing locally is fetched from `refs/pull/<n>/head` first, with the same timeouts as the quality push. One that still isn't there keeps the worktree.
- A unit whose run lock (`ns-run-<unit>.lock`) a live `ns run`, or another cleanup, holds. Nothing is recorded for it either. `ns watch` and `ns clean` take each unit's run lock before they record it, and hold it while they check the worktree again and remove it, so no run starts on the unit meanwhile. `ns run` already holds it when it cleans its own unit. Only `git branch -D` runs under the worktree lock, since it edits `.git/config` as `git worktree add` does; deleting the tree doesn't hold up other units' worktrees.
- The main checkout, or a worktree the command runs from.

`ns watch` tries each merged unit once a night. One that stays, say because `git worktree remove` failed, is tried again by the next night or by `ns clean`, so its records are written once a night at most.

Removal first deletes the phase `TMPDIR`s the unit's failed phases kept (`<system temp dir>/ns-<repo>-<hash>-<unit>/`, read from the `TMPDIR` `ns clean` or `ns watch` inherited, else `/tmp`), so a later failure leaves the unit found with nothing orphaned. Then `git worktree remove --force` (a locked worktree still refuses), then `git branch -D ns/<unit>`. A `TMPDIR` that cannot be removed is reported as kept once the worktree and branch are gone. A worktree on the default branch is detached first, so the default branch is free even if the removal fails, and its event carries `detached`, naming the branch. Each attempt logs a `cleanup` event with `unit`, `path`, `branch`, `issue`, `pr`, `removed`, `reason` when it stayed, `error` when a git command failed, and `by` (`run`, `watch` or `clean`).

`ns clean` reads every worktree on an `ns/` branch, the main checkout included, and every unit worktree on the default branch, and prints `{ok, command, dry_run, status, removed, kept, records}`. `kept` lists every unit that stays, with its reason. `records` is the quality write's event. `status` is `cleaned`, `planned` (a dry run with something to remove), or `nothing-to-clean`, so a rerun is safe. `--dry-run` prints `planned` instead of `removed` and writes nothing: no record, no event, no removal. It still reads issues and PRs with `gh` and may fetch a PR head. It exits 1 when a removal failed.
