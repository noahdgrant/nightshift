# `ns upstream`

## `ns upstream status [--skills-dir <dir>] [--human]`, `ns upstream diff <skill> [--human]`

Compare adapted skills with the upstream each records in `metadata.upstream` (`owner/repo@sha:path`, one or a list). Each repo is fetched with `git` from `https://github.com/owner/repo` into a bare cache at `<git-common-dir>/ns/upstream/owner/repo`: `--depth=1` the first time, plain fetches after that, and `--deepen` (64 commits, doubling) until the cache holds the recorded sha, so a short sha needs no fetch by id. A repo is fetched once per invocation. git's `url.<base>.insteadOf` redirects the fetch, to a mirror or, in the tests, to a local bare repo.

`status` prints `{"ok","changed","skills":[{"skill","upstreams":[{"upstream","head","changed"}]}]}` for every skill with an upstream; `changed` says whether the path differs between the recorded sha and upstream HEAD. An entry that can't be checked (bad format, fetch failed, sha not in HEAD's history) has `error` instead, and the command exits 1. `diff` prints `{"skill","upstreams":[{"upstream","head","diff"}]}`, the path's diff from the recorded sha to HEAD; `--human` prints each diff under `=== <upstream> -> <head>`. An unknown skill, or one with no upstream, exits 2.
