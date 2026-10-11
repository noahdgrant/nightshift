#!/bin/sh
# Install the Rust rule pack into a repo: copy it to .nightshift/rules/rust/ and write the
# `rules` row of docs/agents/stack.md that `ns rules check` runs. Safe to rerun: the pack's own
# files are refreshed, the repo's lists (allowed-crates.txt, roads.txt) are kept. Nothing is
# written until every input has been checked.
#
#   install.sh <repo> [<crate-dir, relative to the repo>]
set -eu

[ $# -ge 1 ] && [ $# -le 2 ] || {
  echo "usage: $0 <repo> [<crate-dir>]" >&2
  exit 2
}
repo=$1
crate=${2:-.}
pack=$(dirname "$0")
dest=.nightshift/rules/rust
stack=$repo/docs/agents/stack.md
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

[ -f "$stack" ] || {
  echo "install.sh: $stack is missing; run ns-setup first" >&2
  exit 2
}

check="$dest/check.sh"
row="| rules | \`$check clippy $crate\`, \`$check deny $crate\`, \`$check ast-grep $crate\` | 1 min |"
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

if [ ! -f "$repo/$dest/allowed-crates.txt" ]; then
  cargo metadata -q --format-version 1 --manifest-path "$repo/$crate/Cargo.toml" \
    >"$tmp/metadata.json" || {
    echo "install.sh: cargo metadata failed for $repo/$crate/Cargo.toml, so the allowlist cannot be written" >&2
    exit 2
  }
  jq -r '.packages[].name' "$tmp/metadata.json" >"$tmp/names"
  sort -u "$tmp/names" >"$tmp/crates"
  {
    echo "# Every crate the dependency graph may hold (cargo-deny bans.allow). Add one only when"
    echo "# no crate already listed does its job."
    cat "$tmp/crates"
  } >"$tmp/allowed-crates.txt"
fi

mkdir -p "$repo/$dest"
cp "$pack/check.sh" "$pack/deny.toml" "$pack/ast-grep.yml" "$pack/clippy-lints.txt" "$repo/$dest/"
[ ! -f "$tmp/allowed-crates.txt" ] || cp "$tmp/allowed-crates.txt" "$repo/$dest/"
if [ ! -f "$repo/$dest/roads.txt" ]; then
  echo "# <name> <file>: the one helper or type that does a job, and the file it lives in." \
    >"$repo/$dest/roads.txt"
fi
cat "$tmp/stack.md" >"$stack"
echo "installed the Rust rule pack in $repo/$dest"
