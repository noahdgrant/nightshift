---
id: D39
title: "ns quality's history lives in the repo, on an orphan data branch nightshift/quality on origin"
issue: 210
date: 2026-10-10
---

# D39: ns quality's history lives in the repo, on an orphan data branch nightshift/quality on origin

## Decision

`ns quality`'s history lives in the repo, on an orphan data branch `nightshift/quality` on `origin`. Its one file, `records.jsonl`, has one JSON line per review attempt: the unit, issue, PR, local day, change size and outcome, and each finding's id, severity, axes, scope, cycle, status and relative location. `ns run` writes a unit's records when it ends `merged`, `done` or `stuck`. It fetches the branch, commits on its tip and pushes, retrying up to 3 times when another writer won the race. If that still fails, the push fails for another reason, or there is no `origin`, the records wait in an outbox under the git common dir until the next write. `ns quality` reads the records first and falls back to a unit's worktree only when the unit has no record. `ns quality import <dir>` turns archived worktrees into records. The branch is data, not code: `ns` writes it unattended, and it is not in `human_review`.

## Reason

Removing a worktree deleted its review artifacts, so the trend existed only on the machine that ran the night, and keeping the 2026-10-09 baseline meant archiving 56 units to a tarball by hand (#210). A branch in the same repo is fetched by every clone, is visible on GitHub, and stays out of `main` and the review flow, so ship's rule that `review.md` matches HEAD is untouched. Rejected: git notes (not fetched by default, hidden on GitHub), commits to `main` (each needs a PR, and the history gets noisy), and PR bodies (the data would live on GitHub, not in the repo). Commits are built with plumbing (`hash-object`, `mktree`, `commit-tree`, then a push of the sha) rather than in a temporary worktree: writing needs no checkout, and no working tree, index or local branch is touched, even with a dirty checkout or a unit mid-run. A rejected push is retried on the new tip, which is a rebase for an append-only file. A unit written twice (stuck, then rerun) counts from its newest write, the latest `recorded` time. Escapes are blamed when the record is written, while the reviewed commit still exists, and the record keeps the commit and PR. Finding titles are left out, since they are free text from a model, and a record holding an absolute path fails a check before it is written; an absolute location is dropped. The record wins over a live worktree, as the issue asked, so a unit rerun after a stuck record shows its old numbers until the rerun ends. A repo with no git identity commits as `nightshift`. Spec: `docs/FACTORY.md` (Quality).
