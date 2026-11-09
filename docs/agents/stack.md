# Stack

What the repository is built with. Read by `ns-triage` and `ns-design-doc`.

- **Language**: Rust, edition 2021, on the toolchain `rust-toolchain.toml` pins (1.95.0, with `clippy` and `rustfmt`).
- **Crate**: one crate, `nightshift`, in `cli/`. It builds the `ns` binary (`cli/src/main.rs`). Dependencies are few: `clap` for the command line, `anyhow` for errors, `serde_json` for the JSON `ns` prints, `toml_edit` for `nightshift.toml`, `globset`, `sha2` and `chrono`.
- **Tools behind a seam**: the harness (Claude Code), the forge (GitHub, through the `gh` CLI), the VCS (git) and the OS (Linux) are traits with one implementation each. Code outside a seam does not name the tool. `docs/GLOSSARY.md` has the words to use.
- **Skills**: `skills/ns-*/SKILL.md`. They are embedded in the binary, so a release pins them.
- **Evals**: `evals/review` holds the cases `ns eval review` runs against the review skill. Each run is a real harness call.
- **Docs**: `docs/` (glossary, coding standards, the state machine in `STATES.md`, versioning). `standards/ns-coding-standards.md` is the language-free standards file `ns` ships; `.nightshift/coding-standards.md` is its copy.
- **Release**: release-please reads Conventional Commit titles and opens the release change request; a Linux binary is attached to each GitHub release.
- **Where to look first**: a state is in `cli/src/states/`, a command in `cli/src/commands/`, the forge calls in `cli/src/forge/`.
