# Verify

How to prove a change works beyond the test suite. `ns-verify` reads this file. There's no verify skill for this repo; the product is a CLI and a set of skills, so drive them directly.

## CLI changes

Build from the worktree, then exercise the changed command against a throwaway git repo in a temp dir, the way a user would:

```bash
cargo build --manifest-path cli/Cargo.toml
T=$(mktemp -d) && git -C "$T" init -q -b main && git -C "$T" commit -q --allow-empty -m init
(cd "$T" && "$OLDPWD/cli/target/debug/ns" <command> ...)
```

Evidence is the exact command, its JSON output, and any files it created. For commands that start a harness or call `gh`, use `--dry-run`, or the fake harness and fake `gh` patterns in `cli/tests/`. Never call a real model or the real GitHub from verification.

## Skill changes

- `cli/target/debug/ns lint skills --human` passes.
- `cli/target/debug/ns eval <skill> --dry-run` plans the skill's cases without errors.
- Read the changed skill end to end as the agent would: every pointer resolves, steps end on a checkable criterion, and the contract (`skills/ns-contract/SKILL.md`) still holds.

## Docs changes

Links resolve, and nothing contradicts `docs/DESIGN.md` decisions or `skills/ns-contract/SKILL.md`.
