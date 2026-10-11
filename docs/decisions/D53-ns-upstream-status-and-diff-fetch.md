---
id: D53
title: "ns upstream status and diff fetch upstream repos into a bare cache"
issue: 49
date: 2026-10-10
---

# D53: ns upstream status and diff fetch upstream repos into a bare cache

## Decision

Builds on D9. `ns upstream status` and `ns upstream diff <skill>` fetch each `metadata.upstream` repo with `git` into a bare cache at `<git-common-dir>/ns/upstream/<owner>/<repo>`: shallow (`--depth=1`) the first time, plain fetches after, then `--deepen` from 64 commits, doubling, until the recorded sha resolves, the history is whole, or 4096 commits are added (then the entry is an error, remembered for the run). Repos come from `https://github.com/<owner>/<repo>`; git's `url.<base>.insteadOf` is the only redirect.

## Reason

`ns-maintain` (#49) needs to know which skills drifted before it opens PRs (#14). Recorded shas are 12-character prefixes, and git can only fetch a commit by its full id, so a fetch by sha can't reach them; deepening can. The cache sits beside the run log and transcripts in the common dir, so every worktree shares it and `git worktree` cleanup never touches it. Using git's own `insteadOf` rather than a new `ns` setting or variable lets a mirror work with no new knob, and lets the tests point at local bare repos through `GIT_CONFIG_*` with no network. Parsing stays in `lint.rs` (`parse_upstream`, `upstream_entries`), so lint and upstream read one format.
