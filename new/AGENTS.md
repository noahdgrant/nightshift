# nightshift, rebuilt

The rebuild lives in `new/`. The code at the repo root is the old version and is read-only reference: copy from it, cut down, and never move or edit it.

- **Language**: `docs/GLOSSARY.md`. Use its words in code, docs and skills, and never a word it lists under _Avoid_.
- **Code**: `docs/CODING_STANDARDS.md`.
- **One meaning in one place.** Point to it instead of restating it.
- **CLI**: `cd new/cli && cargo fmt && cargo clippy --all-targets && cargo test`.
