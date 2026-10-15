# Coding standards

Rules for the `ns` CLI in `cli/`, and the checks that enforce them. Code that a check enforces cites the rule's section: `CODING_STANDARDS.md, "<heading>"`.

## One path per dependency

Each outside program the CLI runs has one way to call it for each job. A new call goes through that path, never a fresh `Command`. When none fits, change the path rather than add a second one.

| Dependency | Job | Path |
|---|---|---|
| git | A command on the local repo | `git::run` (trimmed stdout), `git::ok` (success only), `git::command` (the `Proc`) |
| git | A command that talks to `origin`: `fetch`, `push`, `pull`, `clone`, `ls-remote`, `remote update` | `git::run_net` (trimmed stdout), `git::net` (the `Proc`) |
| A harness (claude, codex, ...) | A headless run: a phase, an eval trial, `ns ask` | `harness::command`, or `harness::capped` under a memory cap; never `Proc::harness` |

`git::run` refuses the network subcommands with an error naming `git::run_net`. The network path never prompts for credentials, and a stalled transfer or a dead ssh connection ends it (#658).

`harness::command` checks a subscription claude's login, drops its `--bare`, turns its auto-memory off and lets its background tasks finish, so no caller can skip one (#657). Clippy rejects `Proc::harness` outside it (`cli/clippy.toml`).

## Checks

These checks enforce "One path per dependency" for child processes:

- Clippy's `disallowed-methods` in `cli/clippy.toml` rejects `std::process::Command::new`, so every child process starts through `crate::proc::Proc`, which applies its kind's policy: the environment scrub, a timeout that kills the process group, stdin closed (#556). It also rejects `Proc::harness`, so every harness starts through `harness::command` (#657).
- `cli/tests/subprocess_guard.rs` (`only_proc_harness_and_the_probe_helper_lift_the_ban_in_src`) fails when a file in `cli/src` other than `proc.rs`, `harness.rs` and the unit tests' probe helper `testutil.rs` allows `clippy::disallowed_methods`.

## A test never waits on the clock

A test waits for an event, never for time to pass: no `sleep`, no timing itself against `Instant::now()` or `.elapsed()`, no literal `Duration` timeout. Code under test that sleeps or reads the time takes `crate::clock::Clock`, which `NS_NOW` pins in a test, so its sleeps advance the pinned clock instead of blocking. A wait that has to bound a hang uses `common::TIMEOUT`.

The ast-grep rule `wall-clock-in-test` in `rules/rust/ast-grep.yml` enforces it (`rules/rust/README.md`). A finding that has to stay, such as a literal that only builds a fake-clock value, goes in the baseline with `ns rules baseline --allow-growth`, which takes a human's yes.

## Lints from review findings

Clippy lints that catch what review kept finding. `ci-local.sh`'s clippy step denies them.

- `cli/Cargo.toml`'s `[lints.clippy]`: `option_option` (an `Option<Option<T>>`, which says nothing about what each `None` means), `fn_params_excessive_bools` and `unused_result_ok` (a `.ok()` whose `Option` is thrown away, dropping the error unread).
- `cli/clippy.toml`'s `max-fn-params-bools = 2`: three or more bool parameters is a signature to replace with an enum or a struct.
