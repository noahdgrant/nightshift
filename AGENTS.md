# nightshift

Agent skills that take an issue to a merged PR (`skills/ns-*`), plus the `ns` CLI in Rust (`cli/`).

- **Writing or editing a skill**: load `skills/ns-writing-for-agents`, then follow `docs/CONVENTIONS.md`. Run `ns lint` before committing.
- **Design and decisions**: `docs/DESIGN.md`. Record a new decision in its Decisions table.
- **Adapted skills**: `metadata.upstream` names the source. Upstream copies are in `NOTICE.md`'s projects. Keep a skill's structure close to its upstream where it still fits, so `ns-maintain` diffs stay readable.
- **CLI**: `cd cli && cargo test`. Every command follows `skills/ns-setup-verify/references/cli-for-agents.md`.
- **Evals**: `docs/EVALS.md`. A skill change comes with eval cases or a reason it has none.
- **CI**: `.github/workflows/ci.yml` runs cargo fmt/clippy/test, `ns lint`, `ns eval --dry-run`, the py-inventory fixture tests, and `scripts/check-private.sh`. The guard's denylist lives in the repo variable `PRIVATE_DENYLIST`; run it locally with `PRIVATE_DENYLIST=a,b scripts/check-private.sh`.
- **Committed files**: generic names and example paths only (`~/zephyrproject/zephyr`), never a private company name or a real home directory path.

## Issues, PRs and labels

Issues live on GitHub at `noahdgrant/nightshift`, managed with `gh`.

- **Every PR closes an issue.** The PR body says `Closes #N`. Work with no issue gets one first.
- **Titles** of issues, PRs and commits are Conventional Commits: `type(scope): subject`, scoped to the skill or `cli`. Append `!` to the type for a breaking change.
- **Labels**, on every issue and PR:
  - exactly one `type:<type>`, matching the Conventional Commit type (`type:feat`, `type:fix`, `type:docs`, `type:refactor`, `type:test`, `type:ci`, ...)
  - at least one `area:*` (`area:cli`, `area:skills`, `area:evals`, `area:factory`, `area:repo` for repo-wide docs and config)
  - `breaking change` when the title has `!`
- **Blockers**: put `Blocked by: #N` on the first line of the issue body.
- **Merging**: squash-merge. The PR title becomes the commit on `main`, so it must be a Conventional Commit.

## Agent skills

This repo runs nightshift on itself.

- **Stack**: `docs/agents/stack.md`
- **Verify**: `docs/agents/verify.md`
- **Issue tracker**: GitHub Issues on `noahdgrant/nightshift`. See `docs/agents/issue-tracker.md`.
- **Triage labels**: `type:*` categories and `status:*` states. See `docs/agents/triage-labels.md`.
- **Domain docs**: `docs/agents/domain.md`
- **Factory definition**: `.nightshift/nightshift.toml`. `ns watch` works `status:ready-for-agent` issues overnight.

