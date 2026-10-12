---
id: ADR-451
title: "Pre-push holds a flock on its shared target dir; ns run's phases push with SSH keepalive"
issue: 451
date: 2026-10-12
amends: [D67]
---

# ADR-451: Pre-push holds a flock on its shared target dir; ns run's phases push with SSH keepalive

## Decision

`.githooks/pre-push` still builds every run in one cargo target dir, `pre-push-target` under the common git dir, or `CARGO_TARGET_DIR` when set (D67). For its whole run it now holds an OS `flock` on `<target dir>.lock`. A push that finds the lock held prints that it is waiting and blocks until the other push finishes. `ci-local.sh` runs with the lock's file descriptor closed, so a process it leaves behind can't keep the lock. The hook exports `NS_PRE_PUSH_LOCK`, the lock it holds, and a run nested inside it with that same lock (this repo's tests run the hook during CI) runs under its parent's lock rather than waiting on it forever. Without `flock` on PATH the hook refuses the push and says how to install it.

`ns run` gives each phase `GIT_SSH_COMMAND="ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=40"` unless `GIT_SSH_COMMAND`, `GIT_SSH` or `core.sshCommand` is already set. `ns doctor` reports `ssh_keepalive` and warns when `origin` is SSH and none of `GIT_SSH_COMMAND`, `core.sshCommand` or the `ssh -G` config sets `ServerAliveInterval`. `docs/FACTORY.md` ("Before the first night") tells the operator to set it.

## Reason

On 2026-10-12 two pushes from different worktrees ran pre-push at once, and one ran the test binaries the other had just built, failing tests its branch did not have. A lock serializes concurrent pushes but leaves a lone push exactly as fast as before, with its incremental cache. A target dir per pushed sha would let pushes run in parallel, but each would build from scratch, and sharing only the dependency build means a second layer of cargo state to keep correct. Parallel units already contend for the same CPU, so running their checks one at a time costs little.

The same morning a push whose hook ran CI for about 15 minutes lost its idle SSH connection and exited 141 after the checks passed. Keepalives keep the connection open; the user's own SSH command still wins.
