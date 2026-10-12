#!/bin/sh
# Install the JavaScript/TypeScript rule pack into a repo: copy it to .nightshift/rules/js/,
# install its tools there with `npm ci`, and write the `rules` row of docs/agents/stack.md that
# `ns rules check` runs. The repo's own package.json and lockfile are never touched. Safe to
# rerun: the pack's own files are refreshed, and the repo's data (utils/allowed-package.yml,
# utils/helper-name.yml and the `ignores` line of rules/duplicate-helper.yml) is kept. The
# first install seeds the allowlist from the repo's package.json. Nothing is written until
# every input has been checked.
#
#   install.sh <repo>
set -eu

[ $# -eq 1 ] || {
  echo "usage: $0 <repo>" >&2
  exit 2
}
repo=$1
pack=$(cd "$(dirname "$0")" && pwd)
dest=.nightshift/rules/js
stack=$repo/docs/agents/stack.md
tmp=$(mktemp -d)
stage=
trap 'rm -rf "$tmp" ${stage:+"$stage"}' EXIT

for t in npm jq; do
  command -v "$t" >/dev/null 2>&1 || {
    echo "install.sh: $t is not on PATH; install it with your package manager (apt install nodejs npm jq)" >&2
    exit 2
  }
done
[ -f "$stack" ] || {
  echo "install.sh: $stack is missing; run ns-setup first" >&2
  exit 2
}

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
grep -qxF "$row" "$tmp/stack.md" || {
  echo "install.sh: $stack has no Commands table to add the rules row to" >&2
  exit 2
}

# The new pack is built beside its destination, so the swap is a rename on one filesystem. It
# holds the repo's data from an earlier install, or else an allowlist of every package the
# repo's package.json names.
old=$repo/$dest
mkdir -p "$(dirname "$old")"
stage=$(mktemp -d "$(dirname "$old")/.js-install.XXXXXX")
backup=$stage/previous
new=$stage/pack
mkdir "$new"
cp -R "$pack/check.sh" "$pack/eslint.config.mjs" "$pack/sgconfig.yml" "$pack/package.json" \
  "$pack/package-lock.json" "$pack/.gitignore" "$pack/rules" "$pack/utils" "$new/"
if [ -f "$old/utils/allowed-package.yml" ]; then
  cp "$old/utils/allowed-package.yml" "$old/utils/helper-name.yml" "$new/utils/"
  ignores=$(grep '^ignores:' "$old/rules/duplicate-helper.yml")
  IGNORES=$ignores awk '/^ignores:/ { print ENVIRON["IGNORES"]; next } { print }' \
    "$pack/rules/duplicate-helper.yml" >"$new/rules/duplicate-helper.yml"
elif [ -f "$repo/package.json" ]; then
  names=$(jq -r '[.name // empty] + ([.dependencies, .devDependencies, .peerDependencies,
      .optionalDependencies] | map(keys? // []) | add) | unique | join("\n")' \
    "$repo/package.json") || {
    echo "install.sh: $repo/package.json is not valid JSON, so the allowlist cannot be written" >&2
    exit 2
  }
  # A package name may hold `.`, the one regex metacharacter npm allows in it.
  alts=$(printf '%s\n' "$names" | sed 's/\./\\./g' | paste -sd'|' -)
  ALTS=$alts awk '/^  regex:/ { printf "  regex: '\''^(?:%s)(?:/|$)'\''\n", ENVIRON["ALTS"]; next } { print }' \
    "$pack/utils/allowed-package.yml" >"$new/utils/allowed-package.yml"
fi

(cd "$new" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error) >"$tmp/npm.log" 2>&1 || {
  cat "$tmp/npm.log" >&2
  echo "install.sh: npm ci failed for the pack's tools" >&2
  exit 2
}

restore() {
  [ ! -e "$backup" ] || { rm -rf "$old"; mv "$backup" "$old"; }
}
if [ -e "$old" ]; then
  mv "$old" "$backup"
fi
mv "$new" "$old" || {
  restore
  echo "install.sh: could not move the new pack to $old; the previous pack is back in place" >&2
  exit 2
}
cat "$tmp/stack.md" >"$stack" || {
  restore
  echo "install.sh: could not write $stack; the previous pack is back in place" >&2
  exit 2
}
echo "installed the JavaScript/TypeScript rule pack in $old"
