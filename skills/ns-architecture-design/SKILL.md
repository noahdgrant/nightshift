---
name: ns-architecture-design
description: How code is structured so a harness can build in it safely: the trodden path first, industry-standard tools, a testable seam at every boundary, and a layout a harness can read. Use when a unit adds a module, a dependency, a boundary to a tool or service, or a second way to do something the repository already does.
---

# Architecture design

Reference only. A harness learns a codebase by reading and copying the code nearest to its change, so the nearest code must be the right code. The design makes the right way the short way.

## Trodden path

- **One way per need.** The repository's own way comes first, then the ecosystem's standard. A change adds no second way: where it replaces one, it moves every caller and deletes the old.
- **When the existing way is the problem**, file a unit for the migration. The change in hand follows the existing way, so the road never forks inside a feature.
- **A new way states its reason**, one line under **Assumptions** in the build summary. Once merged it is the repository's way.
- **Boring over new.** Choose the technology a harness already knows from training and can read the docs of.

## Industry-standard tools

Look for the standard before writing code for a need: the standard library, then the ecosystem's default (in Rust `serde`, `clap`, `tokio`, `anyhow`), then a maintained library. Parsing, retry, configuration, authentication, cryptography, argument handling, migrations and test running each come from one of these.

- **Standard formats and protocols**: JSON, SQL, HTTP, OpenTelemetry, semver, Conventional Commits, not an invented one.
- **A new dependency** states the need it meets, why the standard library and the existing dependencies do not meet it, and that it is maintained and licensed for the use. It enters through the lockfile, and `ns-ci-design` says how versions are pinned.
- **A tool the repository does not own** is called from one place, its seam.

## Testable seams

A **seam** is a place where behaviour changes without editing the code that uses it. Put one at every boundary the code does not own: the network, the filesystem, the clock, randomness, processes, a third-party tool or service.

- **An interface in the domain's words**, one implementation per tool in its own file, chosen in one place. The tool's name, commands and words appear only in that file.
- **Pure logic, I/O at the edge.** Logic takes what it needs as arguments, so a test of it needs no fake.
- **Tests go through the public interface** and swap a fake at the seam, never one inside the unit's own module.
- **Deep, not wide.** A small interface with much behaviour behind it. A seam with one implementation that no test swaps is indirection, so leave it out.

## Harness-friendly layout

- **Named for the thing.** A module holds one concept, named in the glossary's words. `utils`, `helpers` and `common` gather unrelated code, so split by concept, not by layer. The change for one requirement lands in few files.
- **Readable whole.** A file is small enough to read in one go, and a function's name and signature say what it does.
- **Tight feedback.** One command runs the checks (`.nightshift/check`), a fast mode runs in seconds, and a failure names what is wrong and where the rule is written.
- **Fix at the highest layer.** When a harness makes a mistake twice, fix the cause as high as it goes: the codebase (a type that admits only valid values, a boundary), then static analysis (a lint rule, a test, CI), then prose (a comment, a doc, a skill), and last review, the layer that scales worst.
- **Built for the unit.** A layer, option or abstraction is added when the unit needs it, not for a later need nobody has shown.

## In the unit

The build summary names each place the change departs from these and why. A breach is a finding in the Architecture area of the review, at the fix level `code` where a type, boundary or interface shape would prevent it.
