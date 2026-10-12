# nightshift

Agent skills that take an issue to a merged PR (`skills/ns-*`), plus the `ns` CLI in Rust (`cli/`).

- **Writing or editing a skill**: load `skills/ns-writing-for-agents`, then follow `docs/CONVENTIONS.md`. Run `ns lint` before committing.
- **Design and decisions**: `docs/DESIGN.md`. Record a new decision as a file in `docs/decisions/`; `docs/DESIGN.md` ("Decisions") gives its name, ID and format.
- **Adapted skills**: `metadata.upstream` names the source. Upstream copies are in `NOTICE.md`'s projects. Keep a skill's structure close to its upstream where it still fits, so `ns-maintain` diffs stay readable.
- **CLI**: `cd cli && cargo test`. Every command follows `skills/ns-setup-verify/references/cli-for-agents.md`.
- **Evals**: `docs/EVALS.md`. A skill change comes with eval cases or a reason it has none.
- **CI**: `.github/workflows/ci.yml` runs cargo fmt/clippy/test, `ns lint`, `ns eval --dry-run`, the py-inventory fixture tests, the Python rule pack tests (`rules/python/tests/run.sh`), `cargo test` under a decoy `GIT_DIR` (`scripts/check-git-dir-decoy.sh`), and `scripts/check-private.sh`. The guard's denylist lives in the repo variable `PRIVATE_DENYLIST`; run it locally with `PRIVATE_DENYLIST=a,b scripts/check-private.sh`. CI installs its tools from `scripts/ci-tools.toml`, the only place a tool's version is pinned; a test that runs a new tool needs it listed there (`cli/tests/test_tools.rs` checks). `scripts/ci-local.sh` runs all of CI locally, and the `.githooks` hooks run it: pre-commit on the working tree, pre-push in a temporary worktree at each commit pushed. Agents never bypass the hooks (`skills/ns-contract`, Git hooks).
- **Committed files**: generic names and example paths only (`~/zephyrproject/zephyr`), never a private company name or a real home directory path.
- **Tests**: a flaky test is a bug. Reproduce it under load (the test binary in a loop with `--test-threads=16` while busy loops hold the CPU), fix the root cause rather than adding a retry or a longer timeout, and prove the fix with repeated stressed runs.
- **Where knowledge goes**: a harness's per-user memory holds facts about that user only (their accounts, time zone, private context). A rule, convention or decision a contributor needs goes in the repo: this file, `docs/`, or the skill it governs.

## Issues, PRs and labels

Issues live on GitHub at `noahdgrant/nightshift`, managed with `gh`.

- **AI agents mark what they post.** Every issue body, comment and PR body an AI agent posts starts with a 🤖 line naming the agent, then a blank line: `🤖 *Posted by nightshift, an AI agent.*` from nightshift (`skills/ns-contract`, Disclosure), and `🤖 *Posted by an AI agent (Claude Code).*` from an assistant session. Titles and commit messages stay plain Conventional Commits.
- **Every PR closes an issue.** The PR body says `Closes #N`. Work with no issue gets one first.
- **Titles** of issues, PRs and commits are Conventional Commits: `type(scope): subject`, scoped to the skill or `cli`. Append `!` to the type for a breaking change.
- **Labels**, on every issue and PR:
  - exactly one `type:<type>`, matching the Conventional Commit type (`type:feat`, `type:fix`, `type:docs`, `type:refactor`, `type:test`, `type:ci`, ...)
  - at least one `area:*` (`area:cli`, `area:skills`, `area:evals`, `area:factory`, `area:repo` for repo-wide docs and config)
  - `breaking change` when the title has `!`
- **Blockers**: put `Blocked by: #N` on the first line of the issue body, right after the 🤖 line.
- **Design needs a human**: an issue that asks the implementer to choose a design (a mechanism, format, interface or tool) stays `status:needs-define` until the human agrees it. File the build work once the design is settled.
- **Debt before features**: tech debt (review follow-ups, escapes, fixes, refactors, missing tests, CI) is `priority:high`, ahead of new features. See `docs/agents/triage-labels.md`.
- **Merging**: squash-merge. The PR title becomes the commit on `main`, so it must be a Conventional Commit.

## Agent skills

This repo runs nightshift on itself.

- **Stack**: `docs/agents/stack.md`
- **Verify**: `docs/agents/verify.md`
- **Issue tracker**: GitHub Issues on `noahdgrant/nightshift`. See `docs/agents/issue-tracker.md`.
- **Triage labels**: `type:*` categories and `status:*` states. See `docs/agents/triage-labels.md`.
- **Domain docs**: `docs/agents/domain.md`
- **Factory definition**: `.nightshift/nightshift.toml`. `ns watch` works `status:ready-for-agent` issues overnight.

