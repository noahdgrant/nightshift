# Coding standards

A pattern a tool can catch is a check, not a rule here. The rest needs judgement.

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

## No issue numbers in committed files

A committed file never names an issue or change request by number. The number means nothing to a reader who can't see the tracker, and it goes stale when the work moves forge. Say what the issue was about.

Bad:

```rust
// Workaround for #730.
```

Good:

```rust
// A phase can leave read-only directories, which make the workspace unremovable.
```

## The best documentation is no documentation

Code cannot lie; docs can. Write down only the bare minimum, or what is absolutely required. If the code, a type, a test or `--help` already says it, a doc that repeats it is a second copy to keep true.

Bad: a page that lists each command's flags, which `--help` prints.

Good: no page. A doc that stays is one nothing else can say, such as why a choice was made, or a picture of how the parts connect.

## Single source of truth

Keep each meaning in a single place and point to it instead of restating it. A copy can drift from the original, and then the two disagree with no way to tell which is right.

Bad: the list of states in the enum and again in the glossary.

Good: the glossary says "the list is in `docs/STATES.md`".

## Take the well-trodden path

Prefer the conventional tool, library and pattern: the repo's existing choice first, then the ecosystem's standard. Agents do best on ground they have seen most, and they copy what sits nearby. A novel choice needs a stated reason, and once made it becomes the path to follow.

Bad: a hand-rolled argument parser next to commands that use `clap`.

Good: `clap`, like the other commands.
