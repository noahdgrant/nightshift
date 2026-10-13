---
id: D49
title: "ns watch updates its own binary on the nightshift source"
issue: 156
date: 2026-10-10
---

# D49: ns watch updates its own binary on the nightshift source

## Decision

Builds on D46. `ns watch` on the nightshift source (its `cli/Cargo.toml` names the package `nightshift`) updates its own binary. `build.rs` stamps the build commit, which `ns --version` shows. When `origin/<default>` has commits touching `cli/` that the build commit lacks, the night drains: no new claims and no triage pass, running and paused units finish. Once none runs, it builds `cli/` at that commit (`git archive`, then `cargo build --release --locked`) to `<git-common-dir>/ns/self-update/ns-<sha>`, checks that `--version` names the commit and `watch --dry-run` exits 0, and `exec`s it with the same arguments plus a hidden `--resume-night <file>` holding the night's state. A failed build or check keeps the current binary, warns, and is not retried for that commit tonight. Each hand-off logs `self_update` (`from`, `to`). `--no-self-update` turns it off.

## Reason

The stopgap was a shell loop that rebuilt `ns` and restarted `ns watch --max-units 1`, which reset the night's deadline, spend and breakers on every restart (#156). `exec` keeps the pid, so the service manager still sees the same process and the night directory keeps its path; the state file carries what lived only in memory. Waiting for every unit to end means no unit ever runs on two binaries, and the frozen-state rule of D46 holds. `git archive` gives the exact commit without a second worktree to clean up, and `NS_BUILD_COMMIT` passed to the build stamps a tree with no `.git`. The staging path is outside the installed binary, so a broken build never replaces it. Detecting nightshift by its package name rather than the remote URL works for forks and local mirrors. Spec: `docs/factory/starting-a-night.md` ("When the watched repo is nightshift").
