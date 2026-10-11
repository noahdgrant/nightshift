# C rule pack

The rules `ns rules check` runs on a C repo (`docs/FACTORY.md`, "Rules"). The pack composes clang-tidy and ast-grep; it writes no linter of its own. `rules-c` runs one tool per subcommand and prints each finding as a JSON line `ns` reads. It exits 0 with no findings, 1 with findings, and 2 with a `rules-c error:` line, and no findings, when the check could not run.

## Rules

| Rule | Tool | Flags | Enforces |
|---|---|---|---|
| `c-duplicate-function` | ast-grep + `rules-c grep` | a non-static function name defined in more than one file outside `test/` and `tests/` directories (`main` excepted) | [pave the road](../../skills/ns-principle-pave-the-road/SKILL.md); quality bar, Architecture: no near-duplicate helper |
| `c-reinvented-helper` | ast-grep | `sizeof(a) / sizeof(a[0])`, and a function or function-like macro named like `MIN`, `MAX`, `CLAMP`, `ARRAY_SIZE`, `CONTAINER_OF`, with at most a type suffix (`clamp_u8`, `my_max`); constants such as `MAX_RETRIES` pass | [pave the road](../../skills/ns-principle-pave-the-road/SKILL.md); quality bar, Architecture |
| `c-include-not-allowed` | ast-grep + `rules-c grep` | an `#include <...>` the allowlist doesn't name | [pave the road](../../skills/ns-principle-pave-the-road/SKILL.md); quality bar, Architecture: a new dependency states why |
| `c-suppression-without-reason` | ast-grep | `NOLINT`, `NOLINTNEXTLINE`, `NOLINTBEGIN`, `cppcheck-suppress`, `#pragma GCC/clang diagnostic ignored` and `#pragma warning(disable...)` with no reason on the same line | [no comments](../../skills/ns-no-comments/SKILL.md); quality bar, Comments: no suppression hides a real bug |
| `c-file-too-long` | `rules-c size` | a `.c` or `.h` file past 1000 lines | quality bar, Size: no file past roughly 1000 lines |
| `bugprone-*`, `cert-*` | clang-tidy | the groups in `clang-tidy.yaml`, except `bugprone-easily-swappable-parameters` | quality bar, Correctness |
| `readability-*` | clang-tidy | the group, except `readability-magic-numbers` and `readability-identifier-length` | quality bar, Readability; [minimize reader load](../../skills/ns-principle-minimize-reader-load/SKILL.md) |
| `clang-diagnostic-*` | clang-tidy | compiler warnings under `-Wall -Wextra` | quality bar, Correctness and Readability (nothing dead) |

A clang-tidy finding's rule is the check name clang-tidy prints. A check with aliases prints all of them, so its rule is the list (`bugprone-reserved-identifier,cert-dcl37-c,cert-dcl51-cpp`). The three excluded checks flag most firmware code without pointing at a defect. A rule no tool can express, such as a helper duplicated under another name, stays with `ns-review`.

A suppression's reason goes after it on the same line:

```c
x = legacy(); // NOLINT(bugprone-branch-clone): both arms read different registers
#pragma GCC diagnostic ignored "-Wcast-qual" /* the vendor header casts away const */
```

## Compiler warnings

The pack reports `-Wall -Wextra` warnings through clang-tidy. The build should also fail on them: in Zephyr, set `CONFIG_COMPILER_WARNINGS_AS_ERRORS=y` in `prj.conf`; in plain CMake, `target_compile_options(<target> PRIVATE -Wall -Wextra -Werror)`. Turning them on in an existing tree is a separate change from installing the pack, since it may need fixes first.

## Install

Needs `python3` (3.9 or later), `ast-grep` (`pip install ast-grep-cli`, `cargo install ast-grep` or `npm i -g @ast-grep/cli`) and `clang-tidy` 18 (`apt install clang-tidy-18`, then put it on `PATH` as `clang-tidy`), each on `PATH`. The tests assert exact check names and aliases from clang-tidy 18, and CI pins that version. The `ns-setup` skill does these steps:

1. Copy `rules/c/` from the nightshift checkout into the target repo at `.nightshift/rules/c/`, without `tests/`. An installed skill is a symlink into that checkout, so from the target repo: `src="$(realpath <ns-setup skill dir>)/../../rules/c" && mkdir -p .nightshift/rules/c && cp -r "$src/." .nightshift/rules/c && rm -rf .nightshift/rules/c/tests`.
2. Seed the include allowlist with the headers in use today, and review it: `.nightshift/rules/c/rules-c allowlist > .nightshift/rules-c-includes.txt`. Each line is a header (`errno.h`) or a directory prefix ending in `/` (`zephyr/`); `#` starts a comment.
3. Add the `rules` row to `docs/agents/stack.md`. `rules-c tidy` needs the build's `compile_commands.json` (Zephyr writes one in every build directory), so its command builds first, into a temp directory that the subshell's `EXIT` trap removes, keeping the exit status:

   ```markdown
   | rules | `.nightshift/rules/c/rules-c grep`, `.nightshift/rules/c/rules-c size`, `(d=$(mktemp -d) && trap 'rm -rf "$d"' EXIT && west build -b native_sim app -d "$d" >/dev/null 2>&1 && .nightshift/rules/c/rules-c tidy -p "$d")` | 20 s |
   ```

   With `-p`, `rules-c tidy` checks every file in the compile database that sits inside the checkout, tracked or not. Without it, `rules-c tidy` compiles each tracked `.c` file with no flags, which only works for code that needs no include paths. When clang can't compile a file, `rules-c tidy` fails rather than report on half the code. Any `error:` diagnostic counts, including warnings clang makes errors by default, such as an implicit function declaration. `rules-c tidy` passes `-Wno-error`, so a build's `-Werror` still yields findings; a build's `-Werror=<flag>` makes that warning a compile error.
4. Run `ns rules check`. Existing findings go into `.nightshift/rules-baseline.json` with `ns rules baseline --allow-growth`, which a human runs.

Each subcommand checks the paths it is given, else every tracked `.c` and `.h` file outside the pack's own directory. `rules-c grep --allowlist <file>` and `rules-c size --max <n>` override the defaults.

## Tests

Each rule has a failing and a passing case under `tests/cases/<rule>/`. Run them with:

```bash
python3 -m unittest discover -s rules/c/tests
```
