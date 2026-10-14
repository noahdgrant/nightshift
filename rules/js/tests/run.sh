#!/usr/bin/env bash
# Check the JavaScript/TypeScript pack. install.sh installs it into a copy of each fixture repo in
# tests/repos/ (plain JS, TypeScript, React), and the examples in tests/examples/<rule>/ go into
# the repo their extension belongs to (.js, .jsx, .mjs, .cjs: js; .ts, .mts, .cts: ts; .tsx:
# react), or all into the repo a `repo` file in the rule directory names. Each fail* example must
# give a finding for <rule> (the finding's rule, its last /-separated part, or the plugin before
# its first /), and each pass* example no finding at all. A rule directory with a make.sh also
# has it write examples into a temp directory, for examples too long to commit.
#
#   rules/js/tests/run.sh [--find-ns]
set -euo pipefail
shopt -s globstar nullglob dotglob

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

pack=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# The ns this run builds: under $CARGO_TARGET_DIR when set (a relative one is relative to
# the repo root, as in scripts/ci-local.sh), else cli/target. --find-ns only runs it and exits,
# for cli/tests/rules_pack.rs to check this lookup without the whole suite.
target=${CARGO_TARGET_DIR:-cli/target}
case $target in /*) ;; *) target="$pack/../../$target" ;; esac
ns="$target/debug/ns"
if [ "${1:-}" = --find-ns ]; then
  [ -x "$ns" ] || { echo "run.sh: no ns at $ns; build it with: cargo build --manifest-path cli/Cargo.toml" >&2; exit 1; }
  exec "$ns" --version
fi

for tool in node npm jq ast-grep; do
  command -v "$tool" >/dev/null || {
    case $tool in
      ast-grep) how="uv tool install ast-grep-cli" ;;
      *) how="sudo apt-get install nodejs npm jq" ;;
    esac
    echo "run.sh: $tool is not on PATH; install it with: $how" >&2
    exit 1
  }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
failed=0
fail() {
  printf 'FAIL %s\n' "$*" >&2
  failed=1
}
git_as() { git -c user.name=t -c user.email=t@example.com "$@"; }

# A copy of fixture repo $1 at $tmp/$1, a git repo with the pack installed. Its package.json and
# lockfile must come through the install unchanged, and no other lockfile or node_modules may
# appear at its root.
install_repo() {
  local repo=$tmp/$1 out f
  cp -R "$pack/tests/repos/$1" "$repo"
  git -C "$repo" init -q
  mkdir "$tmp/$1.before"
  for f in package.json package-lock.json pnpm-lock.yaml; do
    [ ! -e "$repo/$f" ] || cp "$repo/$f" "$tmp/$1.before/"
  done
  out=$("$pack/install.sh" "$repo" 2>&1) || fail "install.sh $1: $out"
  for f in package.json package-lock.json pnpm-lock.yaml; do
    if [ -e "$tmp/$1.before/$f" ]; then
      git diff --no-index --quiet "$tmp/$1.before/$f" "$repo/$f" || fail "install.sh $1: changed the repo's $f"
    else
      [ ! -e "$repo/$f" ] || fail "install.sh $1: wrote $f at the repo root"
    fi
  done
  [ ! -e "$repo/node_modules" ] || fail "install.sh $1: wrote node_modules at the repo root"
}

# The commands of repo $1's rules row, run from its root; their findings, one JSON line each
# with the leading ./ dropped from the file, go to $tmp/$1.findings. A command that fails with
# no finding is a failure, as in `ns rules check`.
run_row() {
  local repo=$tmp/$1 cmd out rc
  : >"$tmp/$1.findings"
  while IFS= read -r cmd; do
    out=$(cd "$repo" && bash -c "$cmd" 2>"$tmp/stderr") && rc=0 || rc=$?
    [ "$rc" -eq 0 ] || [ -n "$out" ] || fail "$1: \`$cmd\` exited $rc with no finding: $(<"$tmp/stderr")"
    [ -z "$out" ] || jq -c '.file |= ltrimstr("./")' <<<"$out" >>"$tmp/$1.findings"
  done < <(grep '^| rules |' "$repo/docs/agents/stack.md" | grep -o '`[^`]*`' | tr -d '`')
}

# The findings of repo $1 for file $2, relative to the repo root.
findings_for() {
  jq -c --arg f "$2" 'select(.file == $f)' "$tmp/$1.findings"
}

for kind in js ts react ts-include refs pnpm js-tsconfig ts-files entry-points; do
  install_repo "$kind"
done
# The entry-points repo declares its entry points, which turns no-internal-modules on.
entry_points=$tmp/entry-points/.nightshift/rules/js/utils/entry-points.json
printf '["**/index.js"]\n' >"$entry_points"

# Copy each example into its repo, under <rule>/.
mkdir "$tmp/made"
for dir in "$pack"/tests/examples/*/; do
  rule=$(basename "$dir")
  repo_of=
  [ ! -e "$dir/repo" ] || repo_of=$(<"$dir/repo")
  srcs=("$dir")
  if [ -e "$dir/make.sh" ]; then
    mkdir "$tmp/made/$rule"
    "$dir/make.sh" "$tmp/made/$rule"
    srcs+=("$tmp/made/$rule")
  fi
  for src in "${srcs[@]}"; do
    for path in "${src%/}"/**/*.*; do
      case $path in
        *.js | *.jsx | *.mjs | *.cjs) kind=js ;;
        *.ts | *.mts | *.cts) kind=ts ;;
        *.tsx) kind=react ;;
        *) continue ;;
      esac
      kind=${repo_of:-$kind}
      rel=${path#"${src%/}"/}
      mkdir -p "$(dirname "$tmp/$kind/$rule/$rel")"
      cp "$path" "$tmp/$kind/$rule/$rel"
    done
  done
done

# Type-aware rules need a tsconfig.json: the plain JS repo has none, so a floating promise in a
# .ts file there is no finding.
mkdir "$tmp/js/untyped"
printf 'async function save(): Promise<void> {\n  await Promise.resolve();\n}\n\nexport function run(): void {\n  save();\n}\n' \
  >"$tmp/js/untyped/floating.ts"

# A dot directory is scanned; node_modules, .git and the installed pack are not.
printf 'import chalk from "chalk";\nconsole.log(chalk);\n' >"$tmp/js/scan.js"
for d in .hid node_modules/dep .git/hooks .nightshift/rules/js/extra; do
  mkdir -p "$tmp/js/$d"
  cp "$tmp/js/scan.js" "$tmp/js/$d/scan.js"
done

# Files the repo's .gitignore names are not linted, as ast-grep skips them.
printf 'dist/\n' >"$tmp/js/.gitignore"
mkdir "$tmp/js/dist"
printf 'console.log(1);\n' >"$tmp/js/dist/bundle.js"

# A tsconfig.json whose include leaves a .ts file out of the project: that file still gets the
# rules that need no types, and no parse error.
mkdir "$tmp/ts-include/tests"
printf 'export const outside = 1 == 1;\n' >"$tmp/ts-include/tests/outside.test.ts"

# The React rules are on in the React repo for every extension, and off in a repo whose
# package.json has no react. So is no-internal-modules off in a repo that declares no entry
# points.
hooks='import { useState } from "react";\n\nexport function Counter({ on }) {\n  if (!on) return null;\n  const [n] = useState(0);\n  return <img src={String(n)} />;\n}\n'
for kind in react js; do
  mkdir "$tmp/$kind/react-rules"
  for ext in js jsx tsx; do
    # shellcheck disable=SC2059 # the format is the file
    printf "$hooks" >"$tmp/$kind/react-rules/hooks.$ext"
  done
done
cp -R "$pack/tests/examples/no-internal-modules" "$tmp/js/internal-off"

for kind in js ts react ts-include refs pnpm js-tsconfig ts-files entry-points; do
  run_row "$kind"
done

checked=0
for dir in "$pack"/tests/examples/*/; do
  rule=$(basename "$dir")
  nfail=0 npass=0
  for kind in js ts react entry-points; do
    for path in "$tmp/$kind/$rule"/**/*.*; do
      file=${path#"$tmp/$kind/"}
      out=$(findings_for "$kind" "$file")
      case $(basename "$file") in
        fail*)
          nfail=$((nfail + 1))
          jq -e --arg r "$rule" 'select(.rule == $r or (.rule | endswith("/" + $r)) or (.rule | startswith($r + "/")))' <<<"$out" >/dev/null ||
            fail "$rule: $kind/$file gave no $rule finding: $out"
          ;;
        pass*)
          npass=$((npass + 1))
          [ -z "$out" ] || fail "$rule: $kind/$file gave findings: $out"
          ;;
      esac
    done
  done
  [ "$nfail" -gt 0 ] || fail "$rule: no fail* example"
  [ "$npass" -gt 0 ] || fail "$rule: no pass* example"
  checked=$((checked + 1))
done

react_rule='select(.rule | test("^(?:react|react-hooks|jsx-a11y)/"))'
for ext in js jsx tsx; do
  for want in react-hooks/rules-of-hooks jsx-a11y/alt-text; do
    jq -e --arg r "$want" 'select(.rule == $r)' <<<"$(findings_for react "react-rules/hooks.$ext")" >/dev/null ||
      fail "react: no $want finding on react-rules/hooks.$ext"
  done
  ! jq -e "$react_rule" <<<"$(findings_for js "react-rules/hooks.$ext")" >/dev/null ||
    fail "js: a React rule fired with no react in package.json: $(findings_for js "react-rules/hooks.$ext")"
done
! jq -e 'select(.rule | endswith("no-internal-modules"))' <<<"$(findings_for js internal-off/fail.js)" >/dev/null ||
  fail "js: no-internal-modules fired with no entry points declared"
[ -z "$(findings_for js untyped/floating.ts)" ] ||
  fail "a .ts file in a repo with no tsconfig.json got findings: $(findings_for js untyped/floating.ts)"
[ -z "$(findings_for js dist/bundle.js)" ] ||
  fail "a gitignored file was linted: $(findings_for js dist/bundle.js)"
for file in vite.config.ts tests/outside.test.ts; do
  jq -e 'select(.tool == "eslint" and .rule == "eqeqeq")' <<<"$(findings_for ts-include "$file")" >/dev/null ||
    fail "ts-include: no eqeqeq finding on $file outside the tsconfig.json include"
  ! jq -e 'select(.rule == "parse-error")' <<<"$(findings_for ts-include "$file")" >/dev/null ||
    fail "ts-include: $file outside the tsconfig.json include gave a parse error"
done
[ -z "$(findings_for ts-include src/inside.ts)" ] || fail "ts-include: src/inside.ts gave findings"
jq -e 'select(.tool == "eslint" and .rule == "eqeqeq")' <<<"$(findings_for ts-include src/skipped/loose.ts)" >/dev/null ||
  fail "ts-include: no eqeqeq finding on src/skipped/loose.ts, excluded by the tsconfig.json"
! jq -e 'select(.rule == "parse-error")' <<<"$(findings_for ts-include src/skipped/loose.ts)" >/dev/null ||
  fail "ts-include: src/skipped/loose.ts, excluded by the tsconfig.json, gave a parse error"
for file in 'src/[id]/page.ts' src/floating.ts; do
  repo=ts-include
  [ "$file" != src/floating.ts ] || repo=refs
  jq -e 'select(.tool == "eslint" and (.rule | endswith("no-floating-promises")))' <<<"$(findings_for "$repo" "$file")" >/dev/null ||
    fail "$repo: no no-floating-promises finding on $file"
done
for file in vite.config.ts; do
  jq -e 'select(.tool == "eslint" and .rule == "eqeqeq")' <<<"$(findings_for js-tsconfig "$file")" >/dev/null ||
    fail "js-tsconfig: no eqeqeq finding on $file, outside a tsconfig.json project of JavaScript files"
  ! jq -e 'select(.rule == "parse-error")' <<<"$(findings_for js-tsconfig "$file")" >/dev/null ||
    fail "js-tsconfig: $file, outside a tsconfig.json project of JavaScript files, gave a parse error"
done
jq -e 'select(.tool == "eslint" and (.rule | endswith("no-floating-promises")))' <<<"$(findings_for ts-files src/main.ts)" >/dev/null ||
  fail "ts-files: no no-floating-promises finding on src/main.ts, listed in the tsconfig.json files"
jq -e 'select(.tool == "eslint" and .rule == "eqeqeq")' <<<"$(findings_for ts-files src/other.ts)" >/dev/null ||
  fail "ts-files: no eqeqeq finding on src/other.ts, not listed in the tsconfig.json files"
! jq -e 'select(.rule == "parse-error")' <<<"$(findings_for ts-files src/other.ts)" >/dev/null ||
  fail "ts-files: src/other.ts, not listed in the tsconfig.json files, gave a parse error"
jq -e 'select(.tool == "tsc" and .rule == "TS2322")' <<<"$(findings_for refs src/bad.ts)" >/dev/null ||
  fail "refs: no tsc finding on src/bad.ts, checked through a project-references tsconfig.json"
[ -z "$(findings_for pnpm package.json)" ] || fail "pnpm: findings on package.json"
for want in "eslint no-console" "ast-grep dependency-allowlist"; do
  for file in scan.js .hid/scan.js; do
    jq -e --arg t "${want% *}" --arg r "${want#* }" 'select(.tool == $t and .rule == $r)' \
      <<<"$(findings_for js "$file")" >/dev/null || fail "scan: no ${want#* } finding from ${want% *} on $file"
  done
done
for file in node_modules/dep/scan.js .git/hooks/scan.js .nightshift/rules/js/extra/scan.js; do
  [ -z "$(findings_for js "$file")" ] || fail "scan: $file was scanned: $(findings_for js "$file")"
done

# react counts in devDependencies and peerDependencies too. With no package.json, or one that is
# not JSON, the React rules are off and ESLint still runs.
check=$tmp/ts/.nightshift/rules/js/check.sh
for case in devDependencies peerDependencies no-package-json bad-package-json; do
  dir=$tmp/react-$case
  mkdir "$dir"
  # shellcheck disable=SC2059 # the format is the file
  printf "$hooks" >"$dir/hooks.jsx"
  case $case in
    "no-package-json") ;;
    "bad-package-json") printf '{"dependencies": \n' >"$dir/package.json" ;;
    *) printf '{"%s": {"react": "^18.3.0"}}\n' "$case" >"$dir/package.json" ;;
  esac
  out=$("$check" eslint "$dir" 2>"$tmp/stderr") && rc=0 || rc=$?
  hit=$(jq -c 'select(.rule == "react-hooks/rules-of-hooks")' <<<"$out")
  case $case in
    *Dependencies) [ -n "$hit" ] || fail "react in $case: no rules-of-hooks finding: $out $(<"$tmp/stderr")" ;;
    *)
      [ "$rc" -le 1 ] && [ -z "$hit" ] ||
        fail "$case: exit $rc, the React rules ran or ESLint failed: $out $(<"$tmp/stderr")"
      ;;
  esac
done

# An empty or missing entry-points file leaves no-internal-modules off; a malformed one fails
# ESLint with a message naming the file. A rerun of install.sh keeps the repo's entry points.
repo=$tmp/entry-points
internal() { jq -c 'select(.rule == "import-x/no-internal-modules")' <<<"$1"; }
for case in empty missing not-json not-array not-strings; do
  case $case in
    "empty") : >"$entry_points" ;;
    "missing") rm "$entry_points" ;;
    "not-json") printf '["**/index.js"\n' >"$entry_points" ;;
    "not-array") printf '{"allow": ["**/index.js"]}\n' >"$entry_points" ;;
    "not-strings") printf '["**/index.js", 1]\n' >"$entry_points" ;;
  esac
  out=$(cd "$repo" && .nightshift/rules/js/check.sh eslint no-internal-modules 2>"$tmp/stderr") && rc=0 || rc=$?
  case $case in
    "empty" | "missing")
      [ "$rc" -le 1 ] && [ -z "$(internal "$out")" ] ||
        fail "entry-points $case: exit $rc, no-internal-modules ran or ESLint failed: $out $(<"$tmp/stderr")"
      ;;
    *)
      [ "$rc" -eq 2 ] && [ -z "$out" ] && grep -qF "entry-points.json is not" "$tmp/stderr" ||
        fail "entry-points $case: exit $rc, not a clear error: $out $(<"$tmp/stderr")"
      ;;
  esac
done
printf '["**/index.js"]\n' >"$entry_points"
"$pack/install.sh" "$repo" >/dev/null 2>&1 || fail "install.sh: a rerun of the entry-points repo failed"
grep -qF '**/index.js' "$entry_points" || fail "install.sh: a rerun dropped the entry points"
out=$(cd "$repo" && .nightshift/rules/js/check.sh eslint no-internal-modules 2>&1) || true
[ -n "$(internal "$out")" ] || fail "install.sh: no-internal-modules is off after a rerun: $out"

# A rerun keeps the repo's data: the allowlist, the helper names and the duplicate-helper
# ignores. It replaces the rules row rather than adding a second one.
repo=$tmp/js
data=$repo/.nightshift/rules/js
sed -i "s/left-pad/kept-pkg/" "$data/utils/allowed-package.yml"
sed -i "s/mkdirp/keptHelper/" "$data/utils/helper-name.yml"
sed -i 's|^ignores: \[\]|ignores: ["src/kept.js"]|' "$data/rules/duplicate-helper.yml"
"$pack/install.sh" "$repo" >/dev/null 2>&1 || fail "install.sh: a rerun failed"
grep -qF "kept-pkg" "$data/utils/allowed-package.yml" || fail "install.sh: a rerun dropped the allowlist"
grep -qF "keptHelper" "$data/utils/helper-name.yml" || fail "install.sh: a rerun dropped the helper names"
grep -qxF 'ignores: ["src/kept.js"]' "$data/rules/duplicate-helper.yml" ||
  fail "install.sh: a rerun dropped the duplicate-helper ignores"
printf 'ignores:\n  - src/kept.js\n  - src/b.js\n' >"$tmp/ignores"
sed -i -e "/^ignores:/r $tmp/ignores" -e '/^ignores:/d' "$data/rules/duplicate-helper.yml"
"$pack/install.sh" "$repo" >/dev/null 2>&1 || fail "install.sh: a rerun with block ignores failed"
grep -qxF '  - src/b.js' "$data/rules/duplicate-helper.yml" ||
  fail "install.sh: a rerun dropped the block-style duplicate-helper ignores"
[ "$(grep -c '^| rules |' "$repo/docs/agents/stack.md")" -eq 1 ] || fail "install.sh: a rerun added a second rules row"
[ -x "$data/node_modules/.bin/eslint" ] || fail "install.sh: a rerun left no eslint"
mkdir "$repo/src"
printf 'export function keptHelper() {\n  return 1;\n}\n' | tee "$repo/src/kept.js" >"$repo/src/other.js"
out=$(cd "$repo" && .nightshift/rules/js/check.sh ast-grep src 2>&1) || true
jq -e 'select(.file == "src/other.js" and .rule == "duplicate-helper")' <<<"$out" >/dev/null ||
  fail "duplicate-helper: the kept helper name gave no finding on src/other.js: $out"
if grep -qF "kept.js" <<<"$out"; then
  fail "duplicate-helper: the kept ignores did not skip src/kept.js: $out"
fi
sed -i '/^ignores:/d' "$data/rules/duplicate-helper.yml"
"$pack/install.sh" "$repo" >/dev/null 2>&1 || fail "install.sh: a rerun with no ignores line failed"

# An install that fails while swapping the new pack in puts the previous pack back, with the
# repo's data, and leaves no staging directory behind.
mkdir "$tmp/failing-mv"
printf '#!/bin/sh\ncase $1 in */pack) exit 1 ;; esac\nexec /bin/mv "$@"\n' >"$tmp/failing-mv/mv"
chmod +x "$tmp/failing-mv/mv"
repo=$tmp/js
data=$repo/.nightshift/rules/js
cp "$repo/docs/agents/stack.md" "$tmp/stack.before"
PATH=$tmp/failing-mv:$PATH "$pack/install.sh" "$repo" >/dev/null 2>&1 && fail "install.sh: a failing swap was not refused"
grep -qF "kept-pkg" "$data/utils/allowed-package.yml" || fail "install.sh: a failing swap lost the allowlist"
grep -qF "keptHelper" "$data/utils/helper-name.yml" || fail "install.sh: a failing swap lost the helper names"
[ -x "$data/node_modules/.bin/eslint" ] || fail "install.sh: a failing swap lost the previous pack"
git diff --no-index --quiet "$tmp/stack.before" "$repo/docs/agents/stack.md" || fail "install.sh: a failing swap changed stack.md"
left=("$repo"/.nightshift/rules/*)
[ "${#left[@]}" -eq 1 ] || fail "install.sh: a failing swap left a staging directory: ${left[*]}"

# A repo with no package.json gets the pack with the allowlist as shipped, and none is created.
repo=$tmp/no-package-json
mkdir -p "$repo/docs/agents"
cp "$pack/tests/repos/js/docs/agents/stack.md" "$repo/docs/agents/"
"$pack/install.sh" "$repo" >/dev/null 2>&1 || fail "install.sh: a repo with no package.json was refused"
git diff --no-index --quiet "$pack/utils/allowed-package.yml" "$repo/.nightshift/rules/js/utils/allowed-package.yml" ||
  fail "install.sh: a repo with no package.json got a changed allowlist"
[ ! -e "$repo/package.json" ] && [ ! -e "$repo/package-lock.json" ] || fail "install.sh: wrote a package.json or lockfile in a repo with none"

# A stack.md that ends right after the Commands table still gets the rules row, and the
# allowlist is seeded from every dependency field of package.json.
repo=$tmp/table-last
mkdir -p "$repo/docs/agents" "$repo/src"
printf '# Stack\n\n## Commands\n\n| Task | Command | Takes |\n|---|---|---|\n| test | `npm test` | 1 s |\n' >"$repo/docs/agents/stack.md"
printf '{"name": "x", "dependencies": {"dep-a": "1"}, "devDependencies": {"dev-b": "1"}, "peerDependencies": {"peer-c": "1"}, "optionalDependencies": {"opt-d": "1"}}\n' >"$repo/package.json"
printf 'import a from "dep-a";\nimport b from "dev-b";\nimport c from "peer-c";\nimport d from "opt-d";\nimport e from "other-e";\nexport default [a, b, c, d, e];\n' >"$repo/src/imports.js"
"$pack/install.sh" "$repo" >/dev/null 2>&1 || fail "install.sh: a stack.md ending at the Commands table was refused"
[ "$(grep -c '^| rules |' "$repo/docs/agents/stack.md")" -eq 1 ] || fail "install.sh: no rules row after a Commands table at the end of stack.md"
last=$(jq -Rs 'split("\n") | map(select(. != "")) | last' "$repo/docs/agents/stack.md")
[[ $last == '"| rules |'* ]] ||
  fail "install.sh: the rules row is not the last line of a stack.md that ends at the Commands table"
out=$(cd "$repo" && .nightshift/rules/js/check.sh ast-grep src 2>&1) || true
for pkg in dep-a dev-b peer-c opt-d; do
  ! grep -qF "\`$pkg\`" <<<"$out" || fail "install.sh: $pkg, a dependency of package.json, is outside the allowlist: $out"
done
grep -qF "\`other-e\`" <<<"$out" || fail "install.sh: other-e, no dependency of package.json, is inside the allowlist: $out"

# A repo with no Commands table, no stack.md or a broken package.json is refused, as is an
# install whose npm ci fails, and the repo is left as it was.
mkdir -p "$tmp/failing-npm"
printf '#!/bin/sh\nexit 1\n' >"$tmp/failing-npm/npm"
chmod +x "$tmp/failing-npm/npm"
for case in no-table no-stack bad-package-json npm-fails; do
  repo=$tmp/$case
  mkdir -p "$repo/docs/agents"
  printf '{"name": "x"}\n' >"$repo/package.json"
  [ "$case" = no-stack ] || printf '# Stack\n\n## Commands\n\n| Task | Command | Takes |\n|---|---|---|\n' >"$repo/docs/agents/stack.md"
  [ "$case" != no-table ] || printf '# Stack\n\nnone\n' >"$repo/docs/agents/stack.md"
  [ "$case" != bad-package-json ] || printf '{"name": \n' >"$repo/package.json"
  path=$PATH
  [ "$case" != npm-fails ] || path=$tmp/failing-npm:$PATH
  cp -R "$repo" "$tmp/$case.before"
  PATH=$path "$pack/install.sh" "$repo" >/dev/null 2>&1 && fail "install.sh: $case was not refused"
  git diff --no-index --quiet "$tmp/$case.before" "$repo" || fail "install.sh: $case was changed"
done

# check.sh refuses a bad tool or directory, and a tool that fails with no finding shows its
# output on stderr: tsc on a tsconfig.json it can't read.
for args in "lint ." "eslint missing-dir" "eslint"; do
  # shellcheck disable=SC2086 # the words are the arguments
  (cd "$tmp/ts" && "$check" $args >/dev/null 2>&1) && rc=0 || rc=$?
  [ "$rc" -eq 2 ] || fail "check.sh $args: exit $rc, not 2"
done
mkdir "$tmp/broken-tsconfig"
printf '{"compilerOptions": {"target": "nope"}}\n' >"$tmp/broken-tsconfig/tsconfig.json"
out=$("$check" tsc "$tmp/broken-tsconfig" 2>"$tmp/stderr") && rc=0 || rc=$?
[ "$rc" -eq 2 ] && [ -z "$out" ] && grep -qF "TS6046" "$tmp/stderr" && grep -qF "TS18003" "$tmp/stderr" ||
  fail "check.sh tsc: exit $rc on a broken tsconfig.json, stdout: $out, stderr: $(<"$tmp/stderr")"
mkdir "$tmp/failing-ast-grep"
printf '#!/bin/sh\necho "ast-grep broke" >&2\nexit 3\n' >"$tmp/failing-ast-grep/ast-grep"
chmod +x "$tmp/failing-ast-grep/ast-grep"
out=$(cd "$tmp/ts" && PATH=$tmp/failing-ast-grep:$PATH "$check" ast-grep . 2>"$tmp/stderr") && rc=0 || rc=$?
[ "$rc" -eq 3 ] && [ -z "$out" ] && grep -qF "ast-grep broke" "$tmp/stderr" ||
  fail "check.sh ast-grep: exit $rc on a failing tool, stdout: $out, stderr: $(<"$tmp/stderr")"
mkdir -p "$tmp/crashing-tsc/node_modules/.bin"
cp "$check" "$tmp/crashing-tsc/check.sh"
printf '#!/bin/sh\necho "tsc crashed" >&2\nexit 4\n' >"$tmp/crashing-tsc/node_modules/.bin/tsc"
chmod +x "$tmp/crashing-tsc/node_modules/.bin/tsc"
out=$("$tmp/crashing-tsc/check.sh" tsc "$tmp/ts" 2>"$tmp/stderr") && rc=0 || rc=$?
[ "$rc" -eq 4 ] && [ -z "$out" ] && grep -qF "tsc crashed" "$tmp/stderr" ||
  fail "check.sh tsc: exit $rc on a crashing tsc, stdout: $out, stderr: $(<"$tmp/stderr")"
mkdir "$tmp/no-node-modules"
cp "$check" "$tmp/no-node-modules/check.sh"
out=$(cd "$tmp/ts" && "$tmp/no-node-modules/check.sh" eslint . 2>&1) && rc=0 || rc=$?
[ "$rc" -eq 127 ] && grep -qF "npm ci" <<<"$out" || fail "check.sh eslint: exit $rc with no node_modules: $out"

if [ -x "$ns" ]; then
  # The baseline ratchet on the TypeScript repo: its findings are accepted, a new one fails the
  # check, and once fixed, the baseline shrinks without --allow-growth.
  repo=$tmp/ts
  rules() { (cd "$repo" && timeout 120 "$ns" rules "$@" 2>/dev/null); }
  out=$(rules check) && rc=0 || rc=$?
  [ "$rc" -eq 1 ] && jq -e '.new | map(.tool) | index("eslint") and index("tsc") and index("ast-grep")' <<<"$out" >/dev/null ||
    fail "ns rules check: exit $rc on the examples, without eslint, tsc and ast-grep findings: $out"
  out=$(rules baseline --allow-growth) || fail "ns rules baseline --allow-growth: $out"
  out=$(rules check) || fail "ns rules check: the baselined examples failed: $out"
  printf 'export const n = JSON.parse("1") as number;\n' >"$repo/new.ts"
  out=$(rules check) && rc=0 || rc=$?
  [ "$rc" -eq 1 ] && jq -e '.new | length == 1 and .[0].file == "new.ts" and .[0].rule == "@typescript-eslint/consistent-type-assertions"' <<<"$out" >/dev/null ||
    fail "ns rules check: exit $rc on a new violation: $out"
  rm "$repo/new.ts" "$repo/no-floating-promises/fail.ts"
  out=$(rules check) && jq -e '.fixed | length == 1' <<<"$out" >/dev/null ||
    fail "ns rules check: a fixed violation was not reported fixed: $out"
  out=$(rules baseline) && jq -e '.status == "written" and (.dropped | length == 1)' <<<"$out" >/dev/null ||
    fail "ns rules baseline: did not shrink the baseline: $out"
elif [ -n "${CI:-}" ]; then
  fail "ns rules check: $ns is missing; build it with: cargo build --manifest-path cli/Cargo.toml"
else
  echo "run.sh: skipped the ns rules check test; build $ns with: cargo build --manifest-path cli/Cargo.toml" >&2
fi

[ "$failed" -eq 0 ] || exit 1
echo "rules/js: $checked rules, the install and the scan, all examples pass"
