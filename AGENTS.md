# software-factory

Agent skills that take an issue to a merged PR (`skills/sf-*`), plus the `sf` CLI in Rust (`cli/`).

- **Writing or editing a skill**: load `skills/sf-writing-for-agents`, then follow `docs/CONVENTIONS.md`. Run `sf lint` before committing.
- **Design and decisions**: `docs/DESIGN.md`. Record a new decision in its Decisions table.
- **Adapted skills**: `metadata.upstream` names the source. Upstream copies are in `NOTICE.md`'s projects. Keep a skill's structure close to its upstream where it still fits, so `sf-maintain` diffs stay readable.
- **CLI**: `cd cli && cargo test`. Every command follows `skills/sf-setup-verify/references/cli-for-agents.md`.
- **Commits**: Conventional Commits, `type(scope): subject`, with the skill or `cli` as the scope.
