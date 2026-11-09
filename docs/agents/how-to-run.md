# How to run

How to run and verify the repository. Read by `ns-verify`.

The checks are in `.nightshift/check`: it runs `scripts/ci-local.sh`, which runs `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings` and `cargo test --locked` from `cli/`. `scripts/ci-local.sh --fast` skips the tests.

## Build and run the change

- Build: `cargo build --manifest-path cli/Cargo.toml`.
- Run what you built, not the installed `ns`: `NS_DEV=1 cli/target/debug/ns <command>`. `NS_DEV=1` skips the repository's pin, which would otherwise hand the command over to the pinned release.
- Run a command the way a user would. For a change to `ns init`, `ns check`, `ns doctor` or `ns config`, make a scratch git repository in a temporary directory and run the command there.
- When the built `ns` runs in a scratch repository with a fake `gh` on `PATH`, set `GH_TOKEN` to any value, for example `GH_TOKEN=x`. Without it `ns` asks `gh auth token --user <account>` for the account named in the user's config, and a fake `gh` cannot answer. The value is never sent anywhere because `gh` is the fake. Do not use a real token there.
- To build in a temporary copy of the workspace, set `CARGO_TARGET_DIR` to a directory inside the copy (for example `$T/target`), never to `cli/target` in the workspace. Output built from the copy's path stays in the workspace's `target` and breaks its next build once the copy is deleted.

## What the tests do

The integration tests in `cli/tests/cli/` run the real `ns` binary with a fake `gh` that logs its calls, so they show what `ns` asks the forge to do. They do not show what GitHub does.

## Do not

- Do not run `ns run`, `ns auto`, `ns init`, `ns upgrade` or `ns triage` against this repository's own forge to see a change working. They change real issues, labels and change requests. Use a scratch repository and the fakes.
- Do not run a workflow here. Behavior that only GitHub shows (a workflow reacting to a label or a branch protection rule, for example) cannot be proved from a workspace. Say it is unproven, and what would prove it, rather than guessing.
