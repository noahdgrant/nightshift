# Java rule pack

The rules `ns rules check` runs on a Java repo (`docs/factory/run.md`, "Rules"). The pack composes PMD 7 and `ast-grep`, both of which read source only, so it needs no build and works the same under Maven and Gradle. `check.sh` runs one check per subcommand and prints each finding as the JSON line `ns` reads (`{tool, rule, file, line, message}`). It exits 0 with no findings, 1 with findings, and 2 with a `check.sh:` line, and no findings, when the check could not run.

## Rules

| Rule | Tool | Fails on | Enforces |
|---|---|---|---|
| `suppression-reason` | ast-grep + `check.sh` | a `@SuppressWarnings` with no comment after it on the same line, and a `// NOPMD` or `// CHECKSTYLE:OFF` with nothing after the marker | [quality bar](../../skills/ns-contract/references/quality-bar.md), Comments: no suppression hides a real bug |
| `dependency-allowlist` | ast-grep | an import from outside the JDK and the allowlist | [pave the road](../../skills/ns-principle-pave-the-road/SKILL.md): one road per job |
| `duplicate-helper` | ast-grep | a static method named like a helper that already exists | the same |
| `file-size` | `check.sh size` | a `.java` file whose last non-blank line is past line 1000, blank lines at the top included | [quality bar](../../skills/ns-contract/references/quality-bar.md), Size: no file grows past roughly 1000 lines |
| `UnusedPrivateField` | PMD | a private field nothing reads | [quality bar](../../skills/ns-contract/references/quality-bar.md), Readability: nothing is dead: no unused code, parameters or imports |
| `UnusedPrivateMethod` | PMD | a private method nothing calls | the same |
| `UnusedLocalVariable` | PMD | a local variable nothing reads | the same |
| `UnusedFormalParameter` | PMD | a parameter of a private method that the body never uses | the same |
| `EmptyCatchBlock` | PMD | a `catch` block with nothing in it | [quality bar](../../skills/ns-contract/references/quality-bar.md), Correctness: every error is handled or propagated on purpose; nothing is swallowed |
| `OverrideBothEqualsAndHashcode` | PMD | a class that overrides one of `equals` and `hashCode` but not the other | [quality bar](../../skills/ns-contract/references/quality-bar.md), Correctness: a hash collection breaks on such a class |
| `CompareObjectsWithEquals` | PMD | `==` or `!=` between two objects | the same: `==` compares identity, not value |
| `AvoidCatchingThrowable` | PMD | `catch (Throwable t)` | [quality bar](../../skills/ns-contract/references/quality-bar.md), Correctness: errors are handled on purpose, and an `OutOfMemoryError` is not one a caller can handle |
| `ReturnFromFinallyBlock` | PMD | a `return` in a `finally` block | the same: it swallows the exception the `try` threw |

A PMD finding's rule is the PMD rule name, and its message links PMD's page for it. A rule no tool can express, such as a helper duplicated under another name, stays with `ns-review`.

A suppression's reason is a comment after it on the same line:

```java
@SuppressWarnings("unchecked") // the type token checks the cast
int legacy; // NOPMD - read by reflection
// CHECKSTYLE:OFF MagicNumber: the table is data
```

ast-grep can't see a comment beside a node, so `rules/suppression-reason.yml` matches every `@SuppressWarnings` and `check.sh` drops the ones a comment follows on the same line. Any text there counts as the reason; whether it is a good one stays in review.

## Install

The `ns-setup` skill installs the pack, and [its steps](../../skills/ns-setup/references/rules-java.md) work by hand too. Run `install.sh <repo>` from the nightshift checkout. It needs `java` 11 or later (PMD runs on it), `ast-grep`, `jq`, `unzip` and `sha256sum` on `PATH`, and fails with a message naming the missing one. It:

1. downloads the PMD release pinned in `scripts/ci-tools.toml` (`[pmd]`: version, url, sha256) into `${XDG_CACHE_HOME:-~/.cache}/nightshift/`, unless it is already there, and checks its sha256. A zip that doesn't match is removed. `--offline` fails instead of downloading; `scripts/ci-tools.sh install pmd` fills the same cache.
2. copies `check.sh`, `ruleset.xml`, `sgconfig.yml`, `rules/`, `utils/` and PMD (as `pmd/`) to `.nightshift/rules/java/` in the repo, with a `.gitignore` that keeps `pmd/` out of git.
3. seeds the allowlist on the first install (below).
4. adds the row in `stack-row.md` to the Commands table of `docs/agents/stack.md`, with `{pack}` as `.nightshift/rules/java` and `{target}` as `.`, replacing any older `rules` row.

It checks every input before it writes anything. A rerun refreshes the pack and PMD and keeps the repo's data: `utils/allowed-package.yml`, `utils/helper-name.yml` and the `ignores` line of `rules/duplicate-helper.yml`. `.nightshift/**` needs a human merge, so an agent can't loosen a rule or the allowlist to get a unit through.

`check.sh pmd` passes the pack's `ruleset.xml` with `-R`, so a repo's own PMD config can't switch a pack rule off. A file PMD can't parse is an error, not a finding, since a baselined parse error would hide every finding in that file.

Each check scans the `.java` files under its target: in a git work tree the tracked and untracked files git doesn't ignore, else every file outside a dot directory.

## The allowlist

The dependency allowlist is the regex in `utils/allowed-package.yml`. List the repo's own packages and the packages of every dependency it allows, the runtime and the test ones, separated by `|`:

```yaml
rule:
  regex: '^(?:com\.example\.app|com\.fasterxml\.jackson|org\.junit\.jupiter)(?:\.|$)'
```

An entry allows itself and every package under it, so `com.example.app` allows `com.example.app.util` but not `com.example.application`. The first install seeds the list with the packages the repo declares and imports today: each import outside the JDK, cut at its class name, keeping only the shortest of entries that share a prefix. Review it, since it allows whatever the code already uses.

The JDK is always allowed: the packages the `java.*` modules of JDK 21 export, in `utils/jdk-package.yml`, kept by hand. A `javax` package outside the JDK (`javax.inject`, `javax.xml.bind`, `javax.annotation.Nullable`) is not in it.

## Duplicate helpers

`duplicate-helper` flags a static method, never an instance method. It ships with names the JDK already covers: `isBlank` (`String.isBlank`), `requireNonNull` (`Objects.requireNonNull`), `repeat` and `join` (`String`), `readString` (`Files.readString`), `listOf` (`List.of`), `toHexString` (`HexFormat`), `clamp` (`Math.clamp`). Add the repo's own paved helpers to the regex in `utils/helper-name.yml`, and each one's home file to `ignores` in `rules/duplicate-helper.yml`, so the helper itself isn't flagged.

## Tests

`tests/run.sh` installs the pack into a temp repo with `install.sh --offline`, so it needs PMD's zip in the cache (`scripts/ci-tools.sh install pmd`) and never downloads it. It runs the pack once over every example in `tests/examples/<rule>/`: each `fail*.java` must give a finding for that rule, and each `pass*.java` no finding at all. A rule directory with a `make.sh` also gets the examples it writes into a temp directory: `file-size`'s long files, and `dependency-allowlist`'s `pass-jdk-all.java`, which imports every package the running JDK's `java.*` modules export, so the hand-kept list can't miss one. It then tests `install.sh` (the seeded allowlist, a rerun, Java missing, 10 or 11, a missing tool, a bad checksum, no Commands table, and a download from a stand-in pin served over `file://`) and `check.sh` (which files it scans, an inherited `GIT_DIR`, a file PMD can't parse, a broken ast-grep rule), and, when `cli/target/debug/ns` is built, `ns rules check` with the row. `cli/tests/rules_pack_java.rs` tests the baseline on a Java repo. A new rule comes with at least one example of each kind. CI and `scripts/ci-local.sh` run it.

PMD 7.18 deprecated `AvoidCatchingThrowable`, and PMD 8 removes it; it still runs on the pinned 7.28.0, and prints a deprecation warning that `check.sh` keeps off the findings.
