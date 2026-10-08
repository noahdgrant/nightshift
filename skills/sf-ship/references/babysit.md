# Babysit

The loop that takes an open PR to merge-ready. You own this one PR until the gate holds or a human has to act. Commands use `gh`. Swap in the forge CLI named in `docs/agents/issue-tracker.md`.

## Modes

Pick one before the first poll. Default to `drive`.

- `drive`: run the loop to merge-ready. Use it for "babysit this", "get it green", and the end of `sf-ship`.
- `threads-only`: answer review comments and touch nothing else.
- `check`: one status pass and a report. Use it for "is it green?" and for small or docs-only PRs.

## Read the state

```bash
gh pr view <pr> --json mergeable,mergeStateStatus,isDraft,reviewDecision,statusCheckRollup,headRefOid
gh pr checks <pr>
gh api graphql -f query='query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){reviewThreads(first:100){nodes{id isResolved isOutdated comments(first:20){nodes{databaseId author{login} body path line}}}}}}}' -F o=<owner> -F r=<repo> -F n=<pr>
```

Trust the forge's merge state, not a list of green checks. Merge-ready means checks pass, the forge reports the PR mergeable with no conflict, and every review thread is resolved.

## Work in this order

Each wave goes conflicts, then review threads, then CI. Collect every known fix in the wave, then push once. Each push restarts CI, so one push per wave keeps the loop short.

### 1. Conflicts

Resolve a conflict without rewriting published history: `git fetch origin && git merge origin/<base>`, resolve, and re-run the test command from `docs/agents/stack.md`. If the repo forbids merge commits on PR branches, rebase and push with `git push --force-with-lease`, but only while every commit on the branch is yours. If anyone else has pushed to the branch, it is shared. Report the conflict and wait for the human.

After resolving, check for drift. The base may have grown new callers of code this PR moves or deletes. Search for them and fix them in the same wave.

### 2. Review threads

Triage every unresolved thread, from bots and humans alike, with [comment-triage.md](comment-triage.md). Fix real findings in this wave, dismiss noise with a disproof, and escalate the rest.

Push the wave before you reply, so each reply cites the fixing commit. Post replies from a file, so comment text never passes through the shell:

```bash
# payload.json: {"body": "Fixed in abc1234. Added test_timeout_resets_parser, which failed before the fix."}
gh api --method POST repos/<owner>/<repo>/pulls/<pr>/comments/<comment-id>/replies --input payload.json
gh api graphql -f query='mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}' -F id=<thread-id>
```

Resolve a thread after replying, with one exception. When you dismiss or escalate a human's comment, leave the thread open for that human to resolve. Until they do, the PR is waiting on them, which makes the unit `blocked`, not failed.

When feedback drifts from the brief's intent, push back on the thread. Point to the brief instead of growing the PR.

### 3. CI

Classify each failure before you touch it.

- **Failure in code the diff touches**: a real failure. Reproduce it locally, fix it with a failing-first proof (load `sf-tdd`), and add it to the wave.
- **Failure in code the diff never touches**: suspect a stale base first. If `git merge-base --is-ancestor origin/<base> HEAD` fails, bring in the base as in step 1.
- **Flake or infrastructure**: re-run the whole workflow once (`gh run rerun <run-id>`). An identical second failure is not a flake. Reclassify it and read the logs.
- **Hardware-in-the-loop job with no bench available**: report it as `blocked`, naming the job and the bench it needs.

Never weaken, skip, or delete a test to turn CI green. Fix the code, or escalate.

## Wait between waves

After each push, wait on the forge instead of polling in a sleep loop: `gh pr checks <pr> --watch`. Re-read the state and threads when it returns. Review bots often post after CI starts, so re-read threads before you declare merge-ready.

## Stop

- **Merge-ready**: the gate holds. Set `pr.md` to `status: pass` and stop. Leave merging to the human, even if the forge would allow it.
- **Blocked**: a conflict on a shared branch, an escalated finding, a human thread awaiting its owner, a missing bench. Set `status: blocked`, name each blocker, and stop.
- **No progress**: the same check fails after two fix waves, or a third bot pass raises new findings on code you already fixed. Stop as `blocked` and summarize what you tried. Don't churn code to quiet a bot.

A required human approval is a wait, not a blocker to fix. Report it and stop at the gate.

## Report

The mode, the PR and its forge state, each finding fixed or dismissed and why, what is pending, and what needs the human.
