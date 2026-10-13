# Rule packs

The rule packs nightshift ships, one per language. `ns rules check` runs a repo's installed pack (`docs/factory/run.md`, "Rules"), and `ns-setup` installs the pack that matches a repo's stack. `ns lint` checks that this list names exactly the directories under `rules/`, so add a line here when you add a pack.

- [`c/`](c/README.md): C sources (`*.c`, `*.h`), with clang-tidy and ast-grep.
- [`java/`](java/README.md): Java under Maven or Gradle, with PMD and ast-grep on source only.
- [`js/`](js/README.md): JavaScript and TypeScript, React included, with ESLint, `tsc` and ast-grep.
- [`python/`](python/README.md): Python, with ruff, ast-grep and a file-size script.
- [`rust/`](rust/README.md): Rust, with clippy, cargo-deny and ast-grep.
