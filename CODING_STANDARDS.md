# Coding standards

Rules mined from past reviews: the patterns reviewers raised again and again. Each rule cites the units whose review raised it (`454` is the unit for issue #454). `ns-review` checks a diff against this file.

A pattern a tool can catch is a check, not a rule here. The rest needs judgement.

## Checks

These are clippy lints in `cli/Cargo.toml` (`[lints.clippy]`) and `cli/clippy.toml`. The clippy step of `scripts/ci-local.sh` denies them, so a diff that breaks one fails CI.

| Lint | Catches | Units |
|---|---|---|
| `unused_result_ok` | `fs::remove_dir_all(&tmp).ok();`, a statement that drops an error unseen | 454 (four times) |
| `fn_params_excessive_bools`, at 3 or more | `checks_repo(repo, true, false, true)`, a call nobody can read | 488 |
| `option_option` | `Option<Option<Red>>`, where `Some(None)` means green | 466 |

A function doing several jobs is caught in part by `too_many_lines` at 100 lines (425, 488). The Rust rule pack (`rules/rust/clippy-lints.txt`) already runs it, through the `ns rules check` step of `scripts/ci-local.sh`, with today's long functions baselined.

## Pin every branch you add

Every `if` arm, early return, error path and fallback the diff adds needs a test that fails when the branch is removed or flipped. A branch whose mutation survives is untested, whatever the coverage says.

Bad: removing `&& dir.exists()` keeps every test green.

```rust
if keep && dir.exists() {
    fs::remove_dir_all(&dir)?;
}
```

Good: one test per side of each condition.

```rust
#[test]
fn a_missing_dir_is_not_removed() { /* keep = true, dir absent: Ok, no error */ }
#[test]
fn a_kept_dir_is_removed() { /* keep = true, dir present: gone */ }
```

Units: 409, 419, 425, 454, 466, 488.

## A test fails when the behaviour breaks

Assert the outcome the user sees, every time the test runs. No assertion inside an `if`, no check that a message merely contains a word when the whole line is known.

Bad: as root the file is readable, so the test asserts nothing.

```rust
if !readable(&tmp) {
    assert!(exempt.is_empty());
}
```

Good: set up the state the test needs, then assert unconditionally.

```rust
let tmp = unreadable_dir(); // skips with a reason when run as root
assert!(exempt(&tmp).is_empty());
```

Units: 47, 115, 152, 409, 454.

## A test never waits on the clock

A test never sleeps, and never asserts on real elapsed time. It waits for an event: a child exiting, a file appearing, a message on a channel. Or it moves the fake clock forward: `NS_NOW` for a spawned `ns`, `clock::Clock::pinned` in a unit test. A real timeout in a test is only a hang guard: generous, and never the thing the test checks. A test timed by the wall clock passes on an idle machine and fails when units contend for the CPU, which is how the factory always runs.

The Rust rule pack's `wall-clock-in-test` rule (`rules/rust/ast-grep.yml`) flags a sleep, an `Instant::now()` or `.elapsed()` comparison, and a literal `Duration` in test code, with today's cases baselined.

Bad: the test passes only if the child exits within a second.

```rust
let start = Instant::now();
kill(&child);
std::thread::sleep(Duration::from_millis(100));
assert!(start.elapsed() < Duration::from_secs(1));
```

Good: wait for the exit itself, with a hang guard the test never checks.

```rust
kill(&child);
let status = child.wait()?; // the test binary's own timeout is the hang guard
assert!(!status.success());
```

Units: 449, 482, 499, 512, 514.

## Reuse the helper that exists

Before writing a glob matcher, a shell-word splitter, a base-ref lookup or a remove-if-present, search for the one the repo already has. A second copy drifts from the first.

Bad: a third copy of the glob options.

```rust
let opts = MatchOptions { require_literal_separator: true, ..Default::default() };
```

Good: use the one place that owns them, the `MATCH` constant in `cli/src/touches.rs`.

```rust
pat.matches_with(file, MATCH)
```

Units: 47, 419, 425, 454, 466.

## Comments say why, not what

A comment earns its place by saying what the code can't: a constraint, a reason, a trap. Delete a comment that restates the line below it, the file name, or the test's data. The `ns-no-comments` skill does the pass.

Bad:

```rust
// Remove the temp dir.
fs::remove_dir_all(&tmp)?;
```

Good:

```rust
// The phase may still hold files open here on Windows; removal waits for the child to exit.
fs::remove_dir_all(&tmp)?;
```

Units: 152, 409, 425, 454, 466, 473.

## One function, one job

A function that scans, decides and reports is three functions. Split it when you can name the parts; a nested `match` with an early `return` inside a `let` is the sign. The line cap above catches the longest; this rule is for the rest.

Bad:

```rust
fn settle(&self) -> Result<Verdict> {
    let hits = self.scan()?;            // job 1
    let verdict = match hits { /* */ }; // job 2
    self.log(&verdict);                 // job 3
    Ok(verdict)
}
```

Good:

```rust
let verdict = judge(&scan(dir)?);
log(&verdict);
```

Units: 425, 454, 466, 488.

## Name what a value means

A type says what its value means. Use a named struct, not a tuple whose fields can be swapped. Use an enum, not an `Option` whose `None` means something ("run everything") nor a `bool` flag argument. Match every enum case by name, not with `_`.

Bad: `None` means "run every step", and a test can't tell unset from empty.

```rust
fn steps(files: Option<Vec<PathBuf>>) -> Vec<Step>
```

Good:

```rust
enum Scope { All, Files(Vec<PathBuf>) }
fn steps(scope: Scope) -> Vec<Step>
```

Units: 409, 419, 425, 454, 466, 488.
