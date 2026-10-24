# Coding standards

A pattern a tool can catch is a check, not a rule here. The rest needs judgement.

This is what is specific to Rust and to nightshift. The standards that hold in any language (a change request closes a unit, no issue numbers in committed files, one meaning in one place, what to retry) are in `.nightshift/coding-standards.md`, which `ns sync` keeps current and `ns init` points `AGENTS.md` to. `closes-unit.yml` fails a change request that closes none.

## Method names are noun_verb

A method on a trait is named for the thing it acts on, then the action. Methods that act on one thing sort together, and the name says what the thing is before what happens to it.

Bad:

```rust
fn add_workspace(&self, ...) -> Result<()>;
fn merge(&self, ...) -> Result<()>;
```

Good:

```rust
fn workspace_create(&self, ...) -> Result<()>;
fn change_request_merge(&self, ...) -> Result<()>;
```

The noun is a word from `GLOSSARY.md`.

## A tool's name stays in its own module

A seam (`vcs`, `forge`, `harness`, `os`) is a trait with one file per tool. The tool's name, its commands and its words appear only in that file. The trait and everything that uses it speak the glossary's words.

Bad: a command that builds a `Git` and passes it on, or a method called `pull_request`.

Good: `vcs::current()` is the one place that picks a tool, and the method is `change_request_get`.

## The best documentation is code

Code cannot lie; docs can. Write down only the bare minimum, or what is absolutely required. If the code, a type, a test or `--help` already says it, a doc that repeats it is a second copy to keep true.

Bad: a page that lists each command's flags, which `--help` prints.

Good: no page. A doc that stays is one nothing else can say, such as why a choice was made, or a picture of how the parts connect.

## Take the well-trodden path

Prefer the conventional tool, library and pattern: the repo's existing choice first, then the ecosystem's standard. Agents do best on ground they have seen most, and they copy what sits nearby. A novel choice needs a stated reason, and once made it becomes the path to follow.

Bad: a hand-rolled argument parser next to commands that use `clap`.

Good: `clap`, like the other commands.

## Retry in the seam

The shipped standards say what to retry. Here the retry is in the seam that makes the call, with `retry::retry`, and the wait is a field so a test can set it to zero. Cover each kind of failure with a test.

## A test never waits on the clock

A test never sleeps, and never asserts on real elapsed time. It waits for an event: a child exiting, a file appearing, a message on a channel. A real timeout in a test is only a hang guard: generous, and never the thing the test checks.

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

## Comments say why, not what

A comment earns its place by saying what the code can't: a constraint, a reason, a trap. Delete a comment that restates the line below it, the file name, or the test's data.

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
