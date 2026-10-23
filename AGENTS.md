# Nightshift

- **Language**: `docs/GLOSSARY.md`. Use its words in code, docs and skills, and never a word it lists under _Avoid_.
- **Coding standards**: `.claude/skills/ns-coding-standards.md` (any language), then `docs/CODING_STANDARDS.md` (Rust and nightshift).
- **Checks**: `scripts/ci-local.sh` runs what CI runs (cargo fmt, clippy, test); `--fast` skips the tests. Enable the hooks once per clone with `git config core.hooksPath .githooks`: pre-commit runs `--fast`, pre-push runs everything. Never bypass them.
- **Commits and PRs**: titles are Conventional Commits (`type(scope): subject`); release-please reads them to cut `vX.Y.Z` releases.
