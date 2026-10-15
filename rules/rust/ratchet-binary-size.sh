#!/bin/sh
# A ratchet for `ns ratchet`: build binary <bin> of the crate in <crate-dir> (default: the
# current directory) with the release profile and print its size in bytes as the one line on
# stdout. cargo's own output goes to stderr.
#
#   ratchet-binary-size.sh <bin> [<crate-dir>]
set -eu

[ $# -ge 1 ] && [ $# -le 2 ] || {
  echo "usage: $0 <bin> [<crate-dir>]" >&2
  exit 2
}
bin=$1
crate=${2:-.}
for tool in cargo jq; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "ratchet-binary-size.sh: $tool is not on PATH" >&2
    exit 1
  }
done
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cargo build --release --manifest-path "$crate/Cargo.toml" --bin "$bin" \
  --message-format=json-render-diagnostics >"$tmp/messages" || {
  echo "ratchet-binary-size.sh: cargo build failed for $bin in $crate" >&2
  exit 1
}
exe=$(jq -r --arg bin "$bin" \
  'select(.reason == "compiler-artifact" and .target.name == $bin and .executable != null) | .executable' \
  "$tmp/messages")
[ -n "$exe" ] && [ -f "$exe" ] || {
  echo "ratchet-binary-size.sh: cargo built no executable named $bin in $crate" >&2
  exit 1
}
wc -c <"$exe" | tr -d ' '
