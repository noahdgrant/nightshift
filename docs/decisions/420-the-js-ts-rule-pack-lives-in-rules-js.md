---
id: ADR-420
title: "The JavaScript/TypeScript rule pack lives in rules/js/ and installs its own tools"
issue: 420
date: 2026-10-12
amended_by: [ADR-638]
---

# ADR-420: The JavaScript/TypeScript rule pack lives in rules/js/ and installs its own tools

## Decision

Builds on D50, D54 and D55, with the design agreed in #366. The JavaScript/TypeScript rule pack lives in `rules/js/`, and this part holds its core, types, async, complexity, left-behind and other rules; React, module and security rules come later. `check.sh eslint|tsc|ast-grep <dir>` runs one tool and prints D50 JSON lines, as the Rust pack's does, so a finding carries its rule id. ESLint runs with the pack's flat config (`-c`, from `<dir>`) and type-aware typescript-eslint; `tsc --noEmit` runs with the repo's `tsconfig.json`; ast-grep holds `suppression-reason`, `dependency-allowlist`, `duplicate-helper`, `not-implemented` and `skipped-test`. The type-aware rules and `tsc` turn on when `<dir>` has a `tsconfig.json`. `file-size` is ESLint's `max-lines` at 1000. The pack has its own `package.json` and lockfile (ESLint 9, typescript-eslint 8, TypeScript 5), which `install.sh <repo>` installs with `npm ci --ignore-scripts` into `.nightshift/rules/js/`, apart from the repo's own `package.json` and lockfile. The first install seeds the allowlist from the repo's `package.json`; a rerun keeps the allowlist, the helper names and the `ignores` line. `rules/js/tests/run.sh` installs the pack into a plain JS, a TypeScript and a React fixture repo, checks each rule's `fail*` and `pass*` examples and the baseline ratchet, and runs in CI and `scripts/ci-local.sh`. CI pins Node 18 and npm 9.2.0 from Ubuntu's packages in `scripts/ci-tools.toml`, and `ci-tools.sh` now links every apt tool into `/usr/local/bin`, so the runner's own `npm` there can't shadow the pinned one.

## Reason

A lockfile of the pack's own pins its tool versions whatever the repo's package manager, and keeps an agent's unit from adding lint tools to the repo's dependencies. `.nightshift/**` needs a human merge, so an agent can't loosen a rule to get through. `-c` replaces the repo's ESLint config, so its overrides can't switch a pack rule off, and running from `<dir>` makes the config's globs and its `tsconfig.json` test relative to the checked directory. ESLint 9, not 10, because 10 needs Node 20 and Ubuntu 24.04's packaged Node is 18; moving to 10 means pinning a newer Node first. ast-grep parses every file as TSX through `languageGlobs`, so each rule is written once; TSX misreads only `<T>value` assertions in `.ts` files, which `consistent-type-assertions` bans. ESLint's `max-lines` replaces a `file-size.sh` because it already counts every line, blank ones included. A `parse-error` and an `unused-disable-directive` are findings, named by `check.sh` since ESLint gives them no rule id. `ns-setup` got no eval case: its pack step runs `install.sh`, which `run.sh` tests on three fixture repos, and there is no JavaScript eval fixture yet. Spec: `rules/js/README.md`.
