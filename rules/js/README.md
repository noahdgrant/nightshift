# JavaScript/TypeScript rule pack

The rules `ns rules check` runs on a JavaScript or TypeScript repo, React included. They compose ESLint with type-aware [typescript-eslint](https://typescript-eslint.io), `tsc --noEmit` and `ast-grep`, and each one enforces a principle or quality-bar item. `check.sh <tool> <dir>` runs one tool and prints each finding as the JSON line `ns` reads ([ADR-420](../../docs/decisions/420-the-js-ts-rule-pack-lives-in-rules-js.md)). Rules no tool can express stay in review.

One pack covers JavaScript and TypeScript. The type-aware rules, and `tsc`, turn on when the checked directory has a `tsconfig.json`; without one, TypeScript files get only the rules that need no types.

| Rule | Tool | Fails on | Enforces | Upstream |
|---|---|---|---|---|
| `suppression-reason` | ast-grep | an `eslint-disable*` directive with no `-- reason`, or a `@ts-ignore`, `@ts-expect-error` or `@ts-nocheck` with no text after it | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no suppression hides a real bug | Osmani `constraint-driven-development`; pstack `comment-sicko` |
| `ban-ts-comment` | ESLint | a `@ts-ignore` (use `@ts-expect-error`, which fails once the error is gone), or a `@ts-expect-error` with no description | the same | the same |
| `unused-disable-directive` | ESLint | an `eslint-disable` that suppresses nothing (`reportUnusedDisableDirectives`) | the same | |
| `dependency-allowlist` | ast-grep | an `import`, `export ... from`, `require()` or `import()` of a package outside Node's built-ins and the allowlist | [pave the road](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-principle-pave-the-road/SKILL.md): one road per job | Osmani `code-review-and-quality`; pstack `typescript-best-practices` |
| `duplicate-helper` | ast-grep | a function or class named like a helper that already exists | the same | Osmani `code-review-and-quality`; pstack `interrogate`; Pocock `code-review` |
| `max-lines` | ESLint | a file past 1000 lines (the `file-size` rule of the other packs) | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no file grows past roughly 1000 lines | pstack `interrogate` |
| `strict-type-checked` | ESLint | any rule of typescript-eslint's `strict-type-checked` preset (`no-unnecessary-condition`, the `no-unsafe-*` family, ...); `strict` without a `tsconfig.json` | the quality bar's correctness items | pstack `typescript-best-practices`; Osmani `code-review-and-quality` |
| `consistent-type-assertions` | ESLint | any type assertion but `as const`, tests included | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): types say what the code does, and an assertion overrides the checker | pstack `typescript-best-practices`; Pocock `migrate-to-shoehorn` (tests) |
| `switch-exhaustiveness-check` | ESLint | a `switch` on a union that misses a member | the same | pstack `typescript-best-practices` |
| `consistent-type-imports` | ESLint | a type imported without `import type` | the same | |
| `no-floating-promises` | ESLint | a promise nobody awaits, returns or catches | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): every error path is handled | |
| `no-misused-promises` | ESLint | a promise where a boolean or a void-returning callback goes | the same | |
| `no-nested-ternary` | ESLint | a ternary inside a ternary | [minimize reader load](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-principle-minimize-reader-load/SKILL.md) | Osmani `code-simplification` |
| `max-depth` | ESLint | blocks nested past 3 deep | the same | Osmani `code-simplification` |
| `max-params` | ESLint | a function with more than 4 parameters | the same | |
| `not-implemented` | ast-grep | `throw new Error("Not implemented")`, in any case or spacing | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): nothing is left unfinished | Osmani `constraint-driven-development` |
| `no-empty` | ESLint | an empty block, an empty `catch` included | the same, and every error path is handled | Osmani `constraint-driven-development`; pstack `interrogate` |
| `skipped-test` | ast-grep | `.skip` or `.only` anywhere in a member chain rooted at `describe`, `context`, `suite`, `it`, `test` or `bench` (`test.describe.only`, `it.concurrent.skip`), and `xit`, `xdescribe`, `fit` or `fdescribe` called with a title; not Playwright's conditional `test.skip(condition, "reason")`, whose arguments hold no title string or function | the same: a skipped test checks nothing, and `.only` skips the rest | Osmani `constraint-driven-development` (`.skip`) |
| `no-unused-vars` | ESLint | a variable, parameter or import nothing uses (the typescript-eslint rule, which knows type-only uses) | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): nothing is dead | Osmani `code-simplification` |
| `eqeqeq` | ESLint | `==` or `!=` | the quality bar's correctness items: `==` coerces | |
| `no-console` | ESLint | `console.*` outside a `scripts/` directory | the same: output goes through the repo's logger | Osmani `shipping-and-launch`; pstack `typescript-best-practices` |
| `no-eval` | ESLint | `eval(...)` | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no untrusted value reaches `eval` | Osmani `security-and-hardening`; pstack `interrogate` |
| `no-implied-eval` | ESLint | a string passed to `setTimeout`, `setInterval` or `execScript` (typescript-eslint's version of the rule in type-checked files) | the same | the same |
| `no-new-func` | ESLint | `new Function(...)` or `Function(...)`, which evaluate a string as code | the same | the same |
| `inner-html` | ast-grep | assigning `innerHTML` or `outerHTML`, with `=`, `+=`, `["innerHTML"]` or `` [`innerHTML`] ``; reading it is fine. Only static property names written as identifiers or string/template literals are matched; destructuring targets and non-literal keys are not | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no untrusted value reaches a template unescaped | the same |
| `no-danger` | ESLint | `dangerouslySetInnerHTML` (`react/no-danger`); React repos only | the same | the same |
| `rules-of-hooks` | ESLint | a hook called conditionally, in a loop, after an early return, or outside a component or hook; React repos only | the quality bar's correctness items: React matches hooks to state by call order | |
| `exhaustive-deps` | ESLint | an effect, memo or callback whose dependency array misses a value it reads, or lists one it doesn't; React repos only | the same: a stale dependency array runs the hook on old values | |
| `jsx-a11y` | ESLint | any rule of `eslint-plugin-jsx-a11y`'s recommended preset (`alt-text`, `anchor-is-valid`, `click-events-have-key-events`, ...); React repos only | [experience first](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-principle-experience-first/SKILL.md): every user can use the UI | Osmani `frontend-ui-engineering` |
| `no-cycle` | ESLint | an import that leads back to its own file through other imports (`import-x/no-cycle`) | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): dependencies point the right way, with no cycles | Osmani `code-review-and-quality`; Pocock `setup-ts-deep-modules` |
| `no-internal-modules` | ESLint | an import that reaches inside another module past its entry points (`import-x/no-internal-modules`); off until the repo declares entry points | the same: a module is used through its interface | Pocock `setup-ts-deep-modules` |
| `export-star` | ast-grep | `export * from "..."`; not `export * as name from`, which names one namespace | [quality bar](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-contract/references/quality-bar.md): no pass-through layer, and a module's surface is the names it chooses to export | |
| `first` | ESLint | an `import` after other statements (`import-x/first`) | [minimize reader load](https://github.com/noahdgrant/nightshift/blob/main/skills/ns-principle-minimize-reader-load/SKILL.md) | |
| `TS*` | tsc | a type error (`TS2322` and the rest), with the repo's own `tsconfig.json` | the diff's code compiles | |

Upstream names the skill that tells agents to avoid the pattern, in [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills), [mattpocock/skills](https://github.com/mattpocock/skills) or [pstack](https://github.com/cursor/plugins/tree/main/pstack) (`NOTICE.md`); it is blank where none of them does.

A finding's `rule` is the ESLint rule id (`@typescript-eslint/no-floating-promises`), the ast-grep rule id, or the `tsc` code. ESLint reports a file it cannot parse as `parse-error`.

A suppression's reason goes in the same comment:

```ts
// eslint-disable-next-line no-console -- the CLI's only output
// @ts-expect-error: the vendor's types miss the second overload
```

Whether the reason is a good one stays in review.

## Install

The `ns-setup` skill installs the pack with `install.sh <repo>`, and [its steps](../../skills/ns-setup/references/rules-js.md) work by hand too. It copies the pack to `.nightshift/rules/js/`, runs `npm ci` there from the pack's own `package.json` and lockfile, and adds the row in `stack-row.md` to the Commands table of `docs/agents/stack.md`, with `{pack}` and `{target}` filled in. The repo's own `package.json` and lockfile are never touched, whatever its package manager. `.nightshift/**` needs a human merge, so an agent can't loosen a rule or the allowlist to get a unit through.

ESLint runs with `-c` and the pack's `eslint.config.mjs`, from the checked directory, so the repo's own ESLint config is never read and the config's globs are relative to that directory. `tsc` runs with the repo's `tsconfig.json`, since the types are the repo's. Both come from the pack's `node_modules`; `ast-grep` and `jq` must be on `PATH` (`uv tool install ast-grep-cli`). Every tool scans dot directories, and skips `.git`, `node_modules` and the installed pack.

ast-grep parses every file as TSX, so each rule is written once for `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, `.mts`, `.cts` and `.tsx`. TSX misreads a `<T>value` assertion in a `.ts` file, which `consistent-type-assertions` bans anyway.

The React rules (`rules-of-hooks`, `exhaustive-deps`, `jsx-a11y` and `no-danger`) turn on when the checked directory's `package.json` names `react` in `dependencies`, `devDependencies` or `peerDependencies`, as the type-aware ones do with a `tsconfig.json`. They then check every file, `.js` and `.ts` included, since hooks live there too. With no `package.json`, or one that isn't JSON, they stay off.

## Entry points

`no-internal-modules` keeps other code out of a module's insides: an import may reach only a module's entry points. It is off until the repo declares them in `utils/entry-points.json`, the pack's fourth place for the repo's data, as a JSON array of globs:

```json
["**/index.js", "**/index.ts", "src/packages/*/client.ts"]
```

An import that goes one step down (`./lib`, `pkg`) is always fine. One that goes deeper (`./lib/secret.js`, `pkg/dist/x`) fails unless its path, or the file it resolves to, matches a glob; one that resolves to no file is never flagged. The file ships as `[]`; an empty or missing file also leaves the rule off, and one that isn't a JSON array of strings fails the ESLint run with a message naming it.

## The allowlist

The dependency allowlist is the regex in `utils/allowed-package.yml`, one of the four places the pack holds a repo's own data, with `utils/helper-name.yml`, `utils/entry-points.json` and the `ignores` line of `rules/duplicate-helper.yml`; a rerun of `install.sh` keeps all four. The first install fills the allowlist with the repo's package name and every package in its `package.json` (`dependencies`, `devDependencies`, `peerDependencies`, `optionalDependencies`):

```yaml
rule:
  regex: '^(?:my-app|react|@tanstack/react-query|lodash\.merge)(?:/|$)'
```

Add the import prefixes the repo resolves itself: TypeScript `paths` aliases, and the packages of a workspace. Node's built-ins are always allowed: a `node:` specifier, or a name in `utils/node-builtin.yml` (Node 18's `builtinModules`, kept by hand). Relative and absolute paths are never flagged.

## Duplicate helpers

`duplicate-helper` flags functions (declared, or an arrow or function expression assigned to a variable) and classes, never methods. The name match ignores case. It ships with names the platform already covers (`deepClone` for `structuredClone`, `flatten` for `Array.prototype.flat`, `mkdirp` for `fs.mkdirSync(..., { recursive: true })`, `sleep` for `timers/promises`). Add the repo's own paved helpers to the regex in `utils/helper-name.yml`, and each helper's home module to `ignores` in `rules/duplicate-helper.yml`, so the helper itself isn't flagged.

## Tests

`tests/run.sh` installs the pack with `install.sh` into a copy of each fixture repo in `tests/repos/`: plain JavaScript (no `tsconfig.json`), TypeScript, and React. It checks that their `package.json` and lockfile come through unchanged. Each example in `tests/examples/<rule>/` goes into the repo its extension belongs to, or into the repo a `repo` file in the rule's directory names (`no-internal-modules` goes to `entry-points`, which declares entry points), and the repo's `rules` row runs once. Each `fail*` example must give a finding for `<rule>` (the finding's rule, its last `/`-separated part, or the plugin before its first `/`, as `jsx-a11y`), and each `pass*` example no finding at all. Other files in a rule's directory are modules the examples import. A rule directory with a `make.sh` also gets the examples it writes into a temp directory: `max-lines`' long files, and `dependency-allowlist`'s `pass-builtins.mjs`, which imports every built-in of the running Node so the hand-kept list can't miss one. It also checks that the React rules stay off without `react` in `package.json` and `no-internal-modules` without entry points, which directories are scanned, that a rerun keeps the repo's data, and, when `cli/target/debug/ns` is built, the baseline ratchet with `ns rules check` and `ns rules baseline`. A new rule comes with at least one `fail*` and one `pass*` example. CI and `scripts/ci-local.sh` run it.
