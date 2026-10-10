#!/bin/sh
# Install the Rust rule pack into a repo: copy it to .nightshift/rules/rust/ and write the
# `rules` row of docs/agents/stack.md that `ns rules check` runs. Safe to rerun: the pack's own
# files are refreshed, the repo's lists (allowed-crates.txt, roads.txt) are kept.
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

[ -f "$stack" ] || {
  echo "install.sh: $stack is missing; run ns-setup first" >&2
  exit 2
}

mkdir -p "$repo/$dest"
cp "$pack/check.sh" "$pack/deny.toml" "$pack/ast-grep.yml" "$pack/clippy-lints.txt" "$repo/$dest/"
if [ ! -f "$repo/$dest/allowed-crates.txt" ]; then
  {
    echo "# Every crate the dependency graph may hold (cargo-deny bans.allow). Add one only when"
    echo "# no crate already listed does its job."
    cargo metadata -q --format-version 1 --manifest-path "$repo/$crate/Cargo.toml" |
      jq -r '.packages[].name' | sort -u
  } >"$repo/$dest/allowed-crates.txt"
fi
if [ ! -f "$repo/$dest/roads.txt" ]; then
  echo "# <name> <file>: the one helper or type that does a job, and the file it lives in." \
    >"$repo/$dest/roads.txt"
fi

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
' "$stack" >"$stack.tmp"
grep -qxF "$row" "$stack.tmp" || {
  rm -f "$stack.tmp"
  echo "install.sh: $stack has no Commands table to add the rules row to" >&2
  exit 2
}
mv "$stack.tmp" "$stack"
echo "installed the Rust rule pack in $repo/$dest"
