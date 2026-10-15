#!/usr/bin/env bash
# The Rust pack's tests that run outside cargo test: ratchet-binary-size.sh on a small crate,
# and ratchet.toml under `ns ratchet check`. cli/tests/rules_pack.rs tests the rest of the pack.
#
#   rules/rust/tests/run.sh
set -euo pipefail

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

pack=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
target=${CARGO_TARGET_DIR:-cli/target}
case $target in /*) ;; *) target="$pack/../../$target" ;; esac
ns="$target/debug/ns"
script="$pack/ratchet-binary-size.sh"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

failed=0
# Run ratchet-binary-size.sh with these arguments from $tmp; sets out, err and rc.
measure() {
  out=$(cd "$tmp" && "$script" "$@" 2>"$tmp/stderr") && rc=0 || rc=$?
  err=$(<"$tmp/stderr")
}
size_of() {
  python3 -I -c 'import os, sys; print(os.path.getsize(sys.argv[1]))' "$1"
}

# A workspace whose member has two binaries, so the size is the named one's and the target
# directory is the workspace's, not the member's.
crate="$tmp/ws"
mkdir -p "$crate/app/src/bin"
printf '[workspace]\nmembers = ["app"]\nresolver = "2"\n' >"$crate/Cargo.toml"
printf '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n' >"$crate/app/Cargo.toml"
printf 'fn main() {\n    println!("app");\n}\n' >"$crate/app/src/main.rs"
printf 'fn main() {\n    let v: Vec<u64> = (0..100).collect();\n    println!("{:?}", v);\n}\n' >"$crate/app/src/bin/big.rs"

unset CARGO_TARGET_DIR
for bin in app big; do
  measure "$bin" ws/app
  want=$(size_of "$crate/target/release/$bin")
  [ "$rc" -eq 0 ] && [ "$out" = "$want" ] || {
    printf 'FAIL ratchet-binary-size: %s: exit %s, stdout %q, want %s, stderr:\n%s\n' "$bin" "$rc" "$out" "$want" "$err" >&2
    failed=1
  }
done
[ "$(size_of "$crate/target/release/app")" != "$(size_of "$crate/target/release/big")" ] || {
  echo "FAIL ratchet-binary-size: the two binaries are the same size, so the test can't tell them apart" >&2
  failed=1
}

out=$(cd "$crate" && CARGO_TARGET_DIR="$tmp/elsewhere" "$script" app 2>"$tmp/stderr") && rc=0 || rc=$?
want=$(size_of "$tmp/elsewhere/release/app")
[ "$rc" -eq 0 ] && [ "$out" = "$want" ] || {
  printf 'FAIL ratchet-binary-size: CARGO_TARGET_DIR: exit %s, stdout %q, want %s, stderr:\n%s\n' "$rc" "$out" "$want" "$(<"$tmp/stderr")" >&2
  failed=1
}

check_error() {
  local what=$1 want=$2
  [ "$rc" -ne 0 ] && [ -z "$out" ] && grep -qF "$want" <<<"$err" || {
    printf 'FAIL ratchet-binary-size: %s: exit %s, stdout %q, stderr:\n%s\n' "$what" "$rc" "$out" "$err" >&2
    failed=1
  }
}
measure
check_error "no arguments" "usage:"
measure app ws/app extra
check_error "three arguments" "usage:"
measure nosuch ws/app
check_error "an unknown binary" "ratchet-binary-size.sh: cargo build failed for nosuch in ws/app"
measure app nowhere
check_error "a missing crate" "ratchet-binary-size.sh: cargo build failed for app in nowhere"

bare="$tmp/bare-path"
mkdir -p "$bare"
for t in sh mktemp rm jq; do ln -s "$(command -v "$t")" "$bare/$t"; done
out=$(cd "$tmp" && PATH="$bare" "$script" app ws/app 2>"$tmp/stderr") && rc=0 || rc=$?
err=$(<"$tmp/stderr")
check_error "no cargo" "ratchet-binary-size.sh: cargo is not on PATH"

# A cargo that builds and reports no executable for the binary.
printf '#!/bin/sh\necho "{\\"reason\\": \\"build-finished\\", \\"success\\": true}"\n' >"$bare/cargo"
chmod +x "$bare/cargo"
out=$(cd "$tmp" && PATH="$bare" "$script" app ws/app 2>"$tmp/stderr") && rc=0 || rc=$?
err=$(<"$tmp/stderr")
check_error "no executable" "ratchet-binary-size.sh: cargo built no executable named app in ws/app"

if [ -x "$ns" ]; then
  # ratchet.toml, filled in and with the script where the install puts it, is a ratchet
  # `ns ratchet check` runs.
  git -C "$crate" init -q
  mkdir -p "$crate/.nightshift/rules/rust"
  cp "$script" "$crate/.nightshift/rules/rust/"
  sed -e 's|{bin}|app|g' -e 's|{crate}|app|g' "$pack/ratchet.toml" >"$crate/.nightshift/nightshift.toml"
  out=$(cd "$crate" && timeout 300 "$ns" ratchet check 2>/dev/null) && rc=0 || rc=$?
  got=$(jq -c '[.ratchets[] | [.name, .better, .status, (.measured | floor)]]' <<<"$out" 2>/dev/null) || true
  want="[[\"binary-size\",\"lower\",\"untracked\",$(size_of "$crate/target/release/app")]]"
  [ "$rc" -eq 0 ] && [ "$got" = "$want" ] || {
    printf 'FAIL ratchet.toml: ns ratchet check exited %s, want %s:\n%s\n' "$rc" "$want" "$out" >&2
    failed=1
  }
elif [ -n "${CI:-}" ]; then
  echo "FAIL ratchet.toml: $ns is missing; build it with: cargo build --manifest-path cli/Cargo.toml" >&2
  failed=1
else
  echo "run.sh: skipped the ns ratchet check test; build $ns with: cargo build --manifest-path cli/Cargo.toml" >&2
fi

[ "$failed" -eq 0 ] || exit 1
echo "rules/rust: ratchet-binary-size.sh and ratchet.toml pass"
