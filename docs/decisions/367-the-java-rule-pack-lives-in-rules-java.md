---
id: ADR-367
title: "The Java rule pack lives in rules/java/, on PMD 7 and ast-grep"
issue: 367
date: 2026-10-12
amended_by: [ADR-433, ADR-638]
---

# ADR-367: The Java rule pack lives in rules/java/, on PMD 7 and ast-grep

## Decision

Builds on D50 and D55. The Java rule pack lives in `rules/java/` and installs into a repo's `.nightshift/rules/java/` with `install.sh`, which also writes the `rules` row from `stack-row.md`. It composes PMD 7 (`UnusedPrivateField`, `UnusedPrivateMethod`, `UnusedLocalVariable`, `UnusedFormalParameter`, `EmptyCatchBlock`, `OverrideBothEqualsAndHashcode`, `CompareObjectsWithEquals`, `AvoidCatchingThrowable`, `ReturnFromFinallyBlock`, in `ruleset.xml`, passed with `-R`) and `ast-grep` (`suppression-reason`, `dependency-allowlist`, `duplicate-helper`), with `file-size` at 1000 lines. One shell wrapper, `check.sh pmd|ast-grep|size <dir>`, prints D50's JSON lines and exits 2 with no findings when a check can't run, as the Rust and C packs do. The rule IDs are the issue's names without a prefix, as in the Python pack; the `tool` field already says which pack and tool a finding came from.

PMD is pinned once, in `scripts/ci-tools.toml` (`[pmd]`: version, url, sha256). `scripts/ci-tools.sh` gains a `download` method that fetches a zip into `${XDG_CACHE_HOME:-~/.cache}/nightshift/`, checks its sha256 and links its bin onto `PATH`, and an apt `bin` path, for Java 21's JRE. `install.sh` reads the same pin, takes the zip from that cache or downloads it, checks the sha256 on every install, and copies PMD into the repo's pack as `pmd/`, which a `.gitignore` keeps out of git. It needs Java 11 or later on `PATH` and fails naming it. `--offline` fails rather than download, which is how `rules/java/tests/run.sh` and `cli/tests/rules_pack_java.rs` stay off the network: CI fills the cache with `scripts/ci-tools.sh install pmd` first. The pack's per-repo data, kept on a rerun, is the allowlist (`utils/allowed-package.yml`), the helper names (`utils/helper-name.yml`) and the `ignores` line of `rules/duplicate-helper.yml`. The first install seeds the allowlist with the repo's own packages and the packages it imports today. `run.sh` runs in the `rule-pack-java` CI job and `scripts/ci-local.sh`.

## Reason

Agreed in #367. PMD and ast-grep read source only, so the pack needs no build and installs the same under Maven and Gradle; SpotBugs needs compiled classes and Error Prone needs wiring into the build, so both wait. Kotlin waits for its own pack. Passing the pack's ruleset with `-R` means a repo's PMD config can't switch a pack rule off; in-source suppressions still work, and `suppression-reason` makes each one give its reason. A file PMD can't parse is an error rather than a finding, for the reason D56 gives for code clang can't compile: a baselined parse error would hide every finding in that file.

A shell wrapper with `jq` matches the Rust pack; Java repos have no Python to rely on, as Zephyr repos do for the C pack. ast-grep can't see a comment beside a node, so the `@SuppressWarnings` half of `suppression-reason` matches every annotation and `check.sh` drops the ones a comment follows on the same line, read from the match's own source line. The JDK is the packages the `java.*` modules of JDK 21 export, kept by hand like the Python pack's standard library; `run.sh` imports every package the running JDK exports, so the list can't drift unseen. The allowlist is seeded from today's imports, as the C pack seeds its includes, because a Maven `groupId` doesn't name the packages a dependency holds.

PMD's release is about 130 MB, so it is fetched once per machine and verified on every use rather than committed or downloaded per run. `install.sh` reads the pin from `scripts/ci-tools.toml` so the version and checksum live in one place, at the cost of tying the installer to a nightshift checkout, which it already runs from. The checksum was checked against PMD's signed release (`pmd-dist-7.28.0-bin.zip.asc`). PMD 7.18 deprecated `AvoidCatchingThrowable` in favour of `AvoidCatchingGenericException`; it still runs on 7.28.0, so the agreed rule stays until a PMD 8 bump replaces it. `ns-setup` got no eval case: its Java step runs one deterministic installer, which `run.sh` covers, and real eval runs are scheduled separately. Spec: `rules/java/README.md`.
