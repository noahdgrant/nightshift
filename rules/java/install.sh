#!/bin/sh
# Install the Java rule pack into a repo: copy it to .nightshift/rules/java/ with the pinned PMD
# release, and write the `rules` row of docs/agents/stack.md that `ns rules check` runs. Safe to
# rerun: the pack and PMD are refreshed, the repo's lists (utils/allowed-package.yml,
# utils/helper-name.yml, the `ignores` line of rules/duplicate-helper.yml) are kept. Nothing is
# written until every input has been checked.
#
#   install.sh [--offline] <repo>
#
# PMD's version, url and sha256 are the [pmd] pin in scripts/ci-tools.toml. The zip is kept in
# ${XDG_CACHE_HOME:-~/.cache}/nightshift/, where `scripts/ci-tools.sh install pmd` also puts it,
# and checked against the sha256 on every install. --offline fails rather than download it.
set -eu

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

usage() {
  echo "usage: $0 [--offline] <repo>" >&2
  exit 2
}

fail() {
  echo "install.sh: $*" >&2
  exit 2
}

offline=
[ "${1:-}" != --offline ] || { offline=1; shift; }
[ $# -eq 1 ] || usage
repo=$1
pack=$(cd "$(dirname "$0")" && pwd)
dest=.nightshift/rules/java
stack=$repo/docs/agents/stack.md
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

[ -f "$stack" ] || fail "$stack is missing; run ns-setup first"
git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1 || fail "$repo is not a git work tree"

for t in jq ast-grep unzip sha256sum; do
  command -v "$t" >/dev/null 2>&1 || fail "$t is not on PATH; the pack needs it"
done

# PMD runs on the java on PATH.
command -v java >/dev/null 2>&1 ||
  fail "Java is not on PATH, and PMD needs Java 11 or later; install a JDK or JRE (apt install openjdk-21-jre-headless) and rerun"
version=$(java -version 2>&1 | sed -n '1s/^[^"]*"\([^"]*\)".*/\1/p')
major=${version%%[!0-9]*}
[ "${major:-0}" -ge 11 ] ||
  fail "PMD needs Java 11 or later, and the java on PATH is ${version:-of unknown version}; install a newer JDK or JRE (apt install openjdk-21-jre-headless) and rerun"

# The [pmd] table of scripts/ci-tools.toml, with {version} filled in.
pins=$pack/../../scripts/ci-tools.toml
[ -f "$pins" ] || fail "$pins is missing; run install.sh from a nightshift checkout"
pin() {
  awk -v key="$1" '
    /^\[/ { table = ($0 == "[pmd]") }
    table && $1 == key && $2 == "=" { sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit }
  ' "$pins"
}
pmd_version=$(pin version)
url=$(pin url | sed "s/{version}/$pmd_version/g")
sha=$(pin sha256)
bin=$(pin bin | sed "s/{version}/$pmd_version/g")
[ -n "$pmd_version" ] && [ -n "$url" ] && [ -n "$sha" ] && [ -n "$bin" ] ||
  fail "$pins has no [pmd] version, url, sha256 and bin"

cache=${XDG_CACHE_HOME:-$HOME/.cache}/nightshift
archive=$cache/${url##*/}
if [ ! -f "$archive" ]; then
  [ -z "$offline" ] || fail "$archive is missing, and --offline won't download it; fetch it with: scripts/ci-tools.sh install pmd"
  mkdir -p "$cache"
  curl -fsSL --retry 3 -o "$archive.part" "$url" || {
    rm -f "$archive.part"
    fail "cannot download PMD from $url"
  }
  mv "$archive.part" "$archive"
fi
echo "$sha  $archive" | sha256sum -c --status - || {
  rm -f "$archive"
  fail "$archive does not match the sha256 pinned in $pins; removed it, so a rerun downloads it again"
}
unzip -q "$archive" -d "$tmp/unzipped"
[ -x "$tmp/unzipped/$bin" ] || fail "$archive has no $bin"

row=$(sed -e "s|{pack}|$dest|g" -e 's|{target}|.|g' "$pack/stack-row.md")
# Drop any old rules row, then add the new one after the last row of the Commands table.
awk -v row="$row" '
  /^\| *`?rules`? *\|/ { next }
  /^## / { cmds = ($0 == "## Commands") }
  cmds && /^\|/ { table = 1 }
  table && !done && !/^\|/ { print row; done = 1 }
  { print }
  END { if (table && !done) print row }
' "$stack" >"$tmp/stack.md"
grep -qxF "$row" "$tmp/stack.md" || fail "$stack has no Commands table to add the rules row to"

new=$tmp/pack
mkdir "$new"
cp -R "$pack/check.sh" "$pack/ruleset.xml" "$pack/sgconfig.yml" "$pack/rules" "$pack/utils" "$new/"
mv "$tmp/unzipped/${bin%%/*}" "$new/pmd"
echo 'pmd/' >"$new/.gitignore"
old=$repo/$dest
if [ -f "$old/utils/allowed-package.yml" ]; then
  cp "$old/utils/allowed-package.yml" "$old/utils/helper-name.yml" "$new/utils/"
  ignores=$(grep '^ignores:' "$old/rules/duplicate-helper.yml") || fail "$old/rules/duplicate-helper.yml has no ignores line to keep"
  IGNORES=$ignores awk '/^ignores:/ { print ENVIRON["IGNORES"]; next } { print }' \
    "$new/rules/duplicate-helper.yml" >"$tmp/duplicate-helper.yml"
  cp "$tmp/duplicate-helper.yml" "$new/rules/duplicate-helper.yml"
else
  # Seed the allowlist with the repo's own packages and the packages it imports from today:
  # each import the empty allowlist rejects, cut at its first capitalised name (the class).
  (cd "$repo" && "$new/check.sh" ast-grep . >"$tmp/found" 2>"$tmp/log") || [ $? -eq 1 ] ||
    fail "could not list the repo's imports to seed the allowlist: $(cat "$tmp/log")"
  {
    jq -r 'select(.rule == "dependency-allowlist") | .message | capture("^`(?<n>[^`]*)`").n' "$tmp/found"
    git -C "$repo" ls-files -z -co --exclude-standard -- '*.java' | (cd "$repo" && xargs -0 -r cat) |
      sed -n 's/^[[:space:]]*package[[:space:]][[:space:]]*\([A-Za-z0-9_.]*\)[[:space:]]*;.*/\1/p'
  } | awk -F. '{
      name = $1
      for (i = 2; i <= NF && $i !~ /^[A-Z]/; i++) name = name "." $i
      print name
    }' | sort -u | awk '
      # Keep a package only when no package already kept is a prefix of it.
      { for (k in kept) if (index($0 ".", k ".") == 1) next; kept[$0]; print }
    ' >"$tmp/packages"
  regex=$(sed 's/\./\\./g' "$tmp/packages" | paste -sd '|' -)
  REGEX=$regex awk '/^  regex:/ { print "  regex: '"'"'^(?:" ENVIRON["REGEX"] ")(?:\\.|$)'"'"'"; next } { print }' \
    "$new/utils/allowed-package.yml" >"$tmp/allowed-package.yml"
  cp "$tmp/allowed-package.yml" "$new/utils/allowed-package.yml"
fi

mkdir -p "$repo/.nightshift/rules"
rm -rf "$old"
mv "$new" "$old"
cat "$tmp/stack.md" >"$stack"
echo "installed the Java rule pack in $old"
