---
id: D61
title: "ns watch labels a done unit's PR needs-human-merge"
issue: 336
date: 2026-10-11
---

# D61: ns watch labels a done unit's PR needs-human-merge

## Decision

`ns watch` labels the PR of a unit that ended `done` with `[queue] human_merge_label` (default `needs-human-merge`) and comments the merge step's reason with the AI disclaimer. A missing label is created red (`d73a4a`) with a description, never with `--force`, so a colour a human chose stays. A merged unit's PR loses the label if it carries it; a human merge needs no cleanup. The issue gets its status first, and a PR label or comment that fails is reported on stderr and never stops the night (#336).

## Reason

A PR waiting on a human looked like any other open PR; the only signal was the issue's label and a sentence in the PR body, so #319 waited unnoticed. A label on the PR makes the merge queue `gh pr list --label needs-human-merge`. Both `done` branches get it, the plain one under `merge.policy = "human"` and the one that needs a human merge, since each leaves an open PR only a human can merge. GitLab merge requests are left alone until the glab forge work (#322-#324). Cargo tests through the fake `gh` pin the label, the comment, the created label, a custom key, the merged removal, the failure paths, and that `stuck`, `split` and `paused` units touch no PR. Spec: docs/FACTORY.md, cli/README.md.
