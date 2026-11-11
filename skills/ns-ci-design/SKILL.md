---
name: ns-ci-design
description: How a repository's CI pipeline is built: fast and parallel, every version pinned, one command for local and CI, and the checks every change request gets. Use when a unit adds or changes a workflow, a CI check or a script a workflow calls.
---

# CI design

Reference only. CI is the last guardrail before the main branch, so it is fast, repeatable and hard to get around. GitHub Actions is the first forge and the examples are its; on another forge the rules hold in its syntax.

## Every repository has

Three checks, each with a name that never changes and each required by the branch protection: a check that is not required is only advice.

- **Code checks**: format, lint and tests, by the command in `.nightshift/check`.
- **Title check**: the change request's title is a Conventional Commit. Changes squash-merge, so the title becomes the commit on main, and release tooling reads it.
- **Closes check**: the body ends with `Closes #<unit>`, naming a unit that exists. Only the release change request is exempt, known by a branch of this repository and never by a label.

## Fast

- **Parallel**: each independent check is its own job with no `needs`. Use `needs` only where a job consumes another's output, such as a release build that attaches a binary. Checks that share an expensive build (lint and tests on one compile) stay together or share a cache: measure before splitting.
- **Only what the change needs**: a change that touches nothing a check reads skips that check. A markdown-only change runs the title and closes checks and skips the build and the tests. Decide inside the workflow, from the changed files, and skip with a step's `if`, so the required check still reports. Skip by what a check reads, not by extension: a markdown file the build embeds or a test reads still runs them. A changed file nobody listed runs everything.
- **Sharded**: a test suite that grows slow is split across machines, not left to lengthen the check.
- **Cached, never relied on**: a check passes the same with an empty cache, and the key includes the lockfile's hash.
- **Cancelled only when superseded**: a code check on a change request cancels its older runs, and on main never. GitHub counts a cancelled run of a required check as a failure, so a stateless check that takes seconds (title, closes) is never cancelled, and does not trigger on `labeled`.
- **Timed**: every job sets `timeout-minutes`, a few times its normal length. A hung check costs more than a failed one.

## Pinned

A version moves only in a change request someone reviews, whether a person or an update bot opened it. Updating is a choice, never a side effect of a run.

- **Actions**: a full commit SHA with the version in a comment. A tag can be moved to other code, and `@main`, `@stable` and `@latest` are not versions.
- **Toolchains**: from one file in the repository that local and CI both read (`rust-toolchain.toml`, `.nvmrc`, `.tool-versions`), never `stable` or `latest` in the workflow.
- **Packages**: installed from the lockfile, failing when it is out of date (`cargo --locked`, `npm ci`, `uv sync --locked`).
- **Runners and images**: `ubuntu-24.04`, not `ubuntu-latest`; a container by digest.
- **Downloaded tools**: an exact version, checked against a checksum. Never `curl | sh` of the newest.

## One command

A check's logic is in a script the repository owns, which a person can run in a workspace; the workflow sets up and calls it. CI and local run the same command, so a pass in the workspace is a pass in CI. A script has a `--fast` mode for the hook on commit, and the hook on push runs it all. A step only the workflow's YAML can run is behaviour nobody can test before pushing.

## Safe

- **Least privilege**: `permissions: contents: read` at the top, and a job widens it only by what it needs.
- **Untrusted text**: a title, body, branch name or unit text goes in through `env:`, never as `${{ }}` inside a `run:` line.
- **Forks**: their code runs on `pull_request`, with no secrets. Never check out their code under `pull_request_target`.

## Reports

- **A required check always reports**: a workflow skipped by `paths:` or a branch filter never reports, so the merge waits forever. Skip inside it: a job's `if` (a skipped job counts as passing), or a step's `if` when the check must still run, as for a release change request that changes only versions.
- **A job's name is the contract**: protection requires it by name, and a rename quietly drops the requirement. The protection is a setting on the forge, not a file, so the change request says it must change.
- **Retry what can pass**: a check that reads the forge's state may retry a few times, as the closes check does for the forge's lag. A test is never rerun to turn it green: a flaky test gets its own unit.
- **Fail with the rule**: the message names what is wrong and where the rule is written.
