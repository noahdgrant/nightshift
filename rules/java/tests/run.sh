#!/usr/bin/env bash
# Install the Java pack into a temp repo with install.sh, then check each rule against its
# examples in tests/examples/<rule>/: each fail*.java must give a finding for <rule>, and each
# pass*.java no finding at all. A rule directory with a make.sh also has it write examples into
# a temp directory, for examples too long to commit or read from the running JDK. Then it tests
# install.sh and check.sh themselves, and `ns rules check` with the row.
#
# It never downloads PMD: install.sh runs with --offline on the zip that
# `scripts/ci-tools.sh install pmd` keeps in its cache.
#
#   rules/java/tests/run.sh
set -euo pipefail

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

src=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

for tool in java ast-grep jq; do
  command -v "$tool" >/dev/null || {
    echo "run.sh: $tool is not on PATH; install it with: scripts/ci-tools.sh check $tool names how" >&2
    exit 1
  }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
failed=0
fail() {
  printf 'FAIL %s\n' "$@" >&2
  failed=1
}
git_as() { git -c user.name=t -c user.email=t@example.com "$@"; }

# A git repo with a stack.md whose Commands table ends before a Gotchas section, and one source
# file whose package and import seed the allowlist.
new_repo() {
  local repo=$1
  mkdir -p "$repo/docs/agents" "$repo/src/com/example/app"
  git -C "$repo" init -q
  printf '# Stack\n\n## Commands\n\n| Task | Command | Takes |\n|---|---|---|\n| test | `mvn test` | 1 s |\n\n## Gotchas\n\nnone\n' \
    >"$repo/docs/agents/stack.md"
  printf 'package com.example.app;\n\nimport org.junit.jupiter.api.Test;\n\nclass AppTest {\n    @Test\n    void runs() {\n        System.out.println(1);\n    }\n}\n' \
    >"$repo/src/com/example/app/AppTest.java"
}

repo=$tmp/repo
new_repo "$repo"
"$src/install.sh" --offline "$repo" >/dev/null || {
  echo "run.sh: install.sh failed; fetch PMD with: scripts/ci-tools.sh install pmd" >&2
  exit 1
}
pack=$repo/.nightshift/rules/java

# The commands of stack-row.md, the rules row install.sh writes into stack.md, run on target $2
# with pack $1. They exit non-zero when they find something.
findings() {
  local cmd
  while IFS= read -r cmd; do
    cmd=${cmd//\{pack\}/\"\$1\"}
    bash -c "${cmd//\{target\}/\"\$2\"}" _ "$1" "$2" 2>&1 || true
  done < <(grep -o '`[^`]*`' "$src/stack-row.md" | tr -d '`')
}

# Every example goes into one directory tree, so PMD starts once; each file's findings are then
# picked out by its path.
examples=$tmp/examples
checked=0
for dir in "$src"/tests/examples/*/; do
  rule=$(basename "$dir")
  mkdir -p "$examples/$rule"
  cp "$dir"/*.java "$examples/$rule/" 2>/dev/null || true
  [ ! -e "$dir/make.sh" ] || "$dir/make.sh" "$examples/$rule"
  checked=$((checked + 1))
done
out=$(cd "$examples" && findings "$pack" .)
if grep -q '^check.sh:' <<<"$out"; then
  fail "the examples could not be checked:" "$out"
fi
for dir in "$examples"/*/; do
  rule=$(basename "$dir")
  for kind in fail pass; do
    found=0
    for example in "$dir$kind"*.java; do
      [ -e "$example" ] || continue
      found=1
      name=$rule/$(basename "$example")
      mine=$(grep -F "\"file\":\"$name\"" <<<"$out" || true)
      if [ "$kind" = fail ]; then
        grep -qF "\"rule\":\"$rule\"" <<<"$mine" || fail "$rule: $name gave no $rule finding:" "$mine"
      else
        [ -z "$mine" ] || fail "$rule: $name gave findings:" "$mine"
      fi
    done
    [ "$found" -eq 1 ] || fail "$rule: no $kind*.java example"
  done
done

# install.sh seeds the allowlist with the repo's own package and what it imports, ignores PMD in
# git, and adds one rules row after the last row of the Commands table.
grep -qxF "  regex: '^(?:com\\.example\\.app|org\\.junit\\.jupiter\\.api)(?:\\.|\$)'" "$pack/utils/allowed-package.yml" ||
  fail "install: the seeded allowlist is not the repo's package and import:" "$(cat "$pack/utils/allowed-package.yml")"
ignored=$(git -C "$repo" status --porcelain --ignored -- .nightshift | sed -n 's/^!! //p')
grep -qxF .nightshift/rules/java/pmd/bin/pmd <<<"$ignored" && ! grep -qv '^\.nightshift/rules/java/pmd/' <<<"$ignored" ||
  fail "install: git does not ignore exactly the pack's pmd/:" "$ignored"
row=$(sed -e 's|{pack}|.nightshift/rules/java|g' -e 's|{target}|.|g' "$src/stack-row.md")
want=$(printf '| test | `mvn test` | 1 s |\n%s\n\n## Gotchas' "$row")
grep -qzF "$want" "$repo/docs/agents/stack.md" || fail "install: the rules row is not after the test row:" "$(cat "$repo/docs/agents/stack.md")"

# A rerun keeps the repo's lists and replaces the rules row rather than adding a second one.
sed -i 's/(?:com/(?:org\\.kept|com/' "$pack/utils/allowed-package.yml"
sed -i 's/(?:isBlank|/(?:keptHelper|isBlank|/' "$pack/utils/helper-name.yml"
sed -i 's|^ignores: \[\]|ignores: ["**/Home.java"]|' "$pack/rules/duplicate-helper.yml"
cp "$pack/utils/allowed-package.yml" "$pack/utils/helper-name.yml" "$pack/rules/duplicate-helper.yml" "$tmp/"
"$src/install.sh" --offline "$repo" >/dev/null || fail "install: a rerun failed"
for f in utils/allowed-package.yml utils/helper-name.yml rules/duplicate-helper.yml; do
  cmp -s "$pack/$f" "$tmp/$(basename "$f")" || fail "install: a rerun did not keep $f"
done
[ "$(grep -c '^| rules |' "$repo/docs/agents/stack.md")" -eq 1 ] || fail "install: a rerun added a second rules row"

# duplicate-helper skips the files in `ignores`, which is how a helper's own home escapes it.
mkdir "$tmp/ignores"
printf 'final class Strings {\n    static String keptHelper(String s) {\n        return s;\n    }\n}\n' |
  tee "$tmp/ignores/Home.java" >"$tmp/ignores/Other.java"
out=$("$pack/check.sh" ast-grep "$tmp/ignores" 2>&1) || true
grep -qF "\"rule\":\"duplicate-helper\",\"file\":\"$tmp/ignores/Other.java\"" <<<"$out" && ! grep -qF "Home.java" <<<"$out" ||
  fail "duplicate-helper: ignores did not skip exactly Home.java:" "$out"

# install.sh needs Java 11 or later on PATH, and writes nothing without it.
bin=$tmp/no-java-bin
mkdir "$bin"
IFS=: read -ra path_dirs <<<"$PATH"
for d in "${path_dirs[@]}"; do
  [ -d "$d" ] && ln -s "$d"/* "$bin/" 2>/dev/null || true
done
rm -f "$bin/java"
new_repo "$tmp/no-java"
cp "$tmp/no-java/docs/agents/stack.md" "$tmp/stack.md"
out=$(PATH=$bin "$src/install.sh" --offline "$tmp/no-java" 2>&1) && rc=0 || rc=$?
[ "$rc" -eq 2 ] && [ "$out" = "install.sh: Java is not on PATH, and PMD needs Java 11 or later; install a JDK or JRE (apt install openjdk-21-jre-headless) and rerun" ] ||
  fail "install: no java gave exit $rc:" "$out"
mkdir "$tmp/old-java"
printf '#!/bin/sh\necho '\''java version "1.8.0_392"'\'' >&2\n' >"$tmp/old-java/java"
chmod +x "$tmp/old-java/java"
out=$(PATH=$tmp/old-java:$PATH "$src/install.sh" --offline "$tmp/no-java" 2>&1) && rc=0 || rc=$?
[ "$rc" -eq 2 ] && [ "$out" = "install.sh: PMD needs Java 11 or later, and the java on PATH is 1.8.0_392; install a newer JDK or JRE (apt install openjdk-21-jre-headless) and rerun" ] ||
  fail "install: Java 8 gave exit $rc:" "$out"
[ ! -e "$tmp/no-java/.nightshift" ] && cmp -s "$tmp/no-java/docs/agents/stack.md" "$tmp/stack.md" ||
  fail "install: a refused install wrote to the repo"

# --offline with no cached zip says how to fetch it; a cached zip whose checksum differs is
# removed. Neither writes to the repo.
cache=$tmp/cache
out=$(XDG_CACHE_HOME=$cache "$src/install.sh" --offline "$tmp/no-java" 2>&1) && rc=0 || rc=$?
[ "$rc" -eq 2 ] && grep -qF "and --offline won't download it; fetch it with: scripts/ci-tools.sh install pmd" <<<"$out" ||
  fail "install: --offline with no cached zip gave exit $rc:" "$out"
version=$(sed -n '/^\[pmd\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$src/../../scripts/ci-tools.toml")
mkdir -p "$cache/nightshift"
echo 'not PMD' >"$cache/nightshift/pmd-dist-$version-bin.zip"
out=$(XDG_CACHE_HOME=$cache "$src/install.sh" --offline "$tmp/no-java" 2>&1) && rc=0 || rc=$?
[ "$rc" -eq 2 ] && grep -qF "does not match the sha256 pinned in" <<<"$out" && [ ! -e "$cache/nightshift/pmd-dist-$version-bin.zip" ] ||
  fail "install: a zip with the wrong checksum gave exit $rc, or was kept:" "$out"
[ ! -e "$tmp/no-java/.nightshift" ] && cmp -s "$tmp/no-java/docs/agents/stack.md" "$tmp/stack.md" ||
  fail "install: a refused install wrote to the repo"

# A repo without a Commands table, or without a stack.md, is refused.
printf '# Stack\n\nnothing yet\n' >"$tmp/no-java/docs/agents/stack.md"
out=$("$src/install.sh" --offline "$tmp/no-java" 2>&1) && rc=0 || rc=$?
[ "$rc" -eq 2 ] && grep -qF "has no Commands table to add the rules row to" <<<"$out" && [ ! -e "$tmp/no-java/.nightshift" ] ||
  fail "install: a stack.md with no Commands table gave exit $rc:" "$out"
rm "$tmp/no-java/docs/agents/stack.md"
out=$("$src/install.sh" --offline "$tmp/no-java" 2>&1) && rc=0 || rc=$?
[ "$rc" -eq 2 ] && grep -qF "stack.md is missing; run ns-setup first" <<<"$out" || fail "install: no stack.md gave exit $rc:" "$out"
out=$("$src/install.sh" 2>&1) && rc=0 || rc=$?
[ "$rc" -eq 2 ] && grep -qF "usage: " <<<"$out" || fail "install: no repo gave exit $rc:" "$out"

# check.sh scans, in a git repo, the tracked and untracked files git doesn't ignore, and
# elsewhere every file outside a dot directory. Its paths start with the target as given. Every
# check lists the files one way, so the cheap ast-grep check tries each target, and PMD, which
# reads the list from a file, one.
bad='class Bad {\n    private int count;\n    @SuppressWarnings("x")\n    int y;\n}\n'
scanned() {
  local out=$1 rule=$2 label=$3 f
  for f in $want "é and space.java"; do
    grep -qF "\"rule\":\"$rule\",\"file\":\"$dir/$f\"" <<<"$out" || fail "check.sh: $label gave no $rule for $f:" "$out"
  done
  for f in $skip B.txt; do
    ! grep -qF "$f" <<<"$out" || fail "check.sh: $label scanned $f:" "$out"
  done
}
for mode in plain git; do
  dir=$tmp/scan-$mode
  mkdir -p "$dir/.hidden" "$dir/ignored"
  printf "$bad" | tee "$dir/A.java" "$dir/é and space.java" "$dir/.hidden/H.java" "$dir/ignored/I.java" >"$dir/B.txt"
  if [ "$mode" = git ]; then
    git -C "$dir" init -q
    printf 'ignored/\n' >"$dir/.gitignore"
    printf "$bad" >"$dir/Deleted.java"
    git_as -C "$dir" add Deleted.java A.java
    git_as -C "$dir" commit -q -m files
    rm "$dir/Deleted.java"
    want="A.java .hidden/H.java"
    skip="ignored/I.java Deleted.java"
  else
    want="A.java ignored/I.java"
    skip=".hidden/H.java"
  fi
  scanned "$("$pack/check.sh" pmd "$dir" 2>&1)" UnusedPrivateField "$mode pmd"
  for target in "$dir" "$dir/"; do
    out=$("$pack/check.sh" ast-grep "$target" 2>&1) || true
    scanned "$out" suppression-reason "$mode target $target"
    ! grep -qF "$dir//" <<<"$out" || fail "check.sh: $mode target $target doubled a slash:" "$out"
  done
  out=$(cd "$dir" && "$pack/check.sh" ast-grep . 2>&1) || true
  grep -qF '"rule":"suppression-reason","file":"A.java"' <<<"$out" || fail "check.sh: $mode target . gave:" "$out"
done

# An inherited GIT_DIR doesn't redirect check.sh's git, and isn't written to.
decoy=$tmp/decoy
git init -q "$decoy"
git_as -C "$decoy" commit -q --allow-empty -m decoy
before=$(git -C "$decoy" rev-parse HEAD)
out=$(GIT_DIR=$decoy/.git GIT_WORK_TREE=$decoy "$pack/check.sh" ast-grep "$tmp/scan-git" 2>&1) || true
grep -qF "\"file\":\"$tmp/scan-git/.hidden/H.java\"" <<<"$out" || fail "check.sh: an inherited GIT_DIR gave:" "$out"
[ "$(git -C "$decoy" rev-parse HEAD)" = "$before" ] || fail "check.sh: an inherited GIT_DIR was committed to"

# A file PMD can't parse, a missing PMD, a missing target and a bad subcommand each fail with
# exit 2, a check.sh: line and no finding.
mkdir "$tmp/broken"
printf 'class Broken {\n    void f( {\n}\n' >"$tmp/broken/Broken.java"
broken() {
  local want=$1
  shift
  local out err rc
  out=$("$@" 2>"$tmp/err") && rc=0 || rc=$?
  err=$(cat "$tmp/err")
  [ "$rc" -eq 2 ] && [ -z "$out" ] && grep -qF "check.sh: $want" <<<"$err" || fail "check.sh: $* gave exit $rc, want \"$want\":" "$out" "$err"
}
broken "PMD exited 5, so its report is incomplete:" "$pack/check.sh" pmd "$tmp/broken"
grep -qF "Broken.java: ParseException" "$tmp/err" || fail "check.sh: the parse error does not name the file:" "$(cat "$tmp/err")"
cp -R "$pack" "$tmp/no-pmd"
rm -rf "$tmp/no-pmd/pmd"
broken "PMD is not installed in $tmp/no-pmd/pmd; rerun the pack's install.sh" "$tmp/no-pmd/check.sh" pmd "$tmp/scan-plain"
broken "$tmp/nowhere is not a directory" "$pack/check.sh" size "$tmp/nowhere"
out=$("$pack/check.sh" lint . 2>&1) && rc=0 || rc=$?
[ "$rc" -eq 2 ] && grep -qF "usage: " <<<"$out" || fail "check.sh: an unknown check gave exit $rc:" "$out"

# A directory with no Java files has no findings and exits 0.
mkdir "$tmp/empty"
for check in pmd ast-grep size; do
  out=$("$pack/check.sh" "$check" "$tmp/empty" 2>&1) && rc=0 || rc=$?
  [ "$rc" -eq 0 ] && [ -z "$out" ] || fail "check.sh: $check on an empty directory gave exit $rc:" "$out"
done

# The ns this run builds: under $CARGO_TARGET_DIR when set (a relative one is relative to the
# repo root, as in scripts/ci-local.sh), else cli/target.
target=${CARGO_TARGET_DIR:-cli/target}
case $target in /*) ;; *) target="$src/../../$target" ;; esac
ns="$target/debug/ns"
if [ -x "$ns" ]; then
  printf 'class Bare {\n    int count; // NOPMD\n}\n' >"$repo/src/Bare.java"
  out=$(cd "$repo" && timeout 120 "$ns" rules check 2>/dev/null) && rc=0 || rc=$?
  [ "$rc" -eq 1 ] && grep -qF '"findings": 1,' <<<"$out" && grep -qF '"file": "src/Bare.java"' <<<"$out" && grep -qF '"rule": "suppression-reason"' <<<"$out" ||
    fail "ns rules check: exit $rc on a bare NOPMD gave:" "$out"
elif [ -n "${CI:-}" ]; then
  fail "ns rules check: $ns is missing; build it with: cargo build --manifest-path cli/Cargo.toml"
else
  echo "run.sh: skipped the ns rules check test; build $ns with: cargo build --manifest-path cli/Cargo.toml" >&2
fi

[ "$failed" -eq 0 ] || exit 1
echo "rules/java: $checked rules, install.sh and check.sh, all examples pass"
