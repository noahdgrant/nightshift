#!/usr/bin/env bash
# The Rust pack's tests that run outside cargo test: ratchet-binary-size.sh on a small crate,
# ratchet.toml under `ns ratchet check`, and the file scopes of the harness-kind-construction and
# tree-removal-skips-modes rules. cli/tests/rules_pack.rs tests the rest of the pack.
#
#   rules/rust/tests/run.sh
set -euo pipefail

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX
# The gate under `ns run` sets these for its own ratchets; the test repos have their own.
unset NS_RATCHET_FACTORY NS_RATCHET_STEADY NS_RATCHET_REPORT

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

# A cargo that builds and reports a library and no executable.
cat >"$bare/cargo" <<'EOF_CARGO'
#!/bin/sh
echo '{"reason": "compiler-artifact", "target": {"name": "app"}, "executable": null}'
echo '{"reason": "build-finished", "success": true}'
EOF_CARGO
chmod +x "$bare/cargo"
out=$(cd "$tmp" && PATH="$bare" "$script" app ws/app 2>"$tmp/stderr") && rc=0 || rc=$?
err=$(<"$tmp/stderr")
check_error "no executable" "ratchet-binary-size.sh: cargo built no executable named app in ws/app"

nojq="$tmp/no-jq-path"
mkdir -p "$nojq"
for t in sh mktemp rm; do ln -s "$(command -v "$t")" "$nojq/$t"; done
cp "$bare/cargo" "$nojq/cargo"
out=$(cd "$tmp" && PATH="$nojq" "$script" app ws/app 2>"$tmp/stderr") && rc=0 || rc=$?
err=$(<"$tmp/stderr")
check_error "no jq" "ratchet-binary-size.sh: jq is not on PATH"
[ "$err" = "ratchet-binary-size.sh: jq is not on PATH" ] || {
  printf 'FAIL ratchet-binary-size: no jq: stderr not exact:\n%s\n' "$err" >&2
  failed=1
}

# harness-kind-construction flags a Kind::Harness built outside the files allowed to build one,
# and never a pattern that matches it.
cat >"$tmp/kind.rs" <<'EOF_KIND'
fn f(k: Kind, s: bool) -> u8 {
    let _ = Kind::Harness { subscription: true }; // built
    let _ = proc::Kind::Harness { subscription: s }; // built
    let _ = Kind::Harness { subscription }; // built
    let _ = MyKind::Harness { subscription };
    match k {
        Kind::Harness { .. } => 1,
        _ => 0,
    }
}

fn g(k: Kind) {
    if let Kind::Harness { subscription: true } = k {}
    if let proc::Kind::Harness { .. } = k {}
}
EOF_KIND
built=()
n=0
while IFS= read -r l; do
  n=$((n + 1))
  [[ $l != *'// built'* ]] || built+=("$n")
done <"$tmp/kind.rs"
kinds="$tmp/kinds"
for f in other.rs harness.rs proc.rs proc/tests.rs nested/harness.rs; do
  mkdir -p "$(dirname "$kinds/cli/src/$f")"
  cp "$tmp/kind.rs" "$kinds/cli/src/$f"
done
out=$(cd "$kinds" && "$pack/check.sh" ast-grep cli 2>"$tmp/stderr") || true
got=$(jq -rs 'map(select(.rule == "harness-kind-construction")) | sort_by(.file, .line)
  | map("\(.file) \(.line)") | join(" ")' <<<"$out" 2>/dev/null) || true
want=
for f in nested/harness.rs other.rs; do
  for l in "${built[@]}"; do want+="${want:+ }cli/src/$f $l"; done
done
[ "$got" = "$want" ] || {
  printf 'FAIL harness-kind-construction: findings %q, want %q, stderr:\n%s\n' "$got" "$want" "$(<"$tmp/stderr")" >&2
  failed=1
}
jq -e -s 'map(select(.rule == "harness-kind-construction")) | length > 0 and all(.message | contains("crate::harness::command"))' <<<"$out" >/dev/null 2>&1 || {
  echo "FAIL harness-kind-construction: a finding's message does not name crate::harness::command" >&2
  failed=1
}

# tree-removal-skips-modes flags remove_dir_all in the cleanup files outside test code, and nowhere
# else.
cat >"$tmp/rm.rs" <<'EOF_RM'
fn f(p: &Path) {
    let _ = fs::remove_dir_all(p); // flagged
    let _ = std::fs::remove_dir_all(p); // flagged
    let _ = crate::tree::remove(p);
}

#[cfg(test)]
mod tests {
    fn g(p: &Path) {
        let _ = fs::remove_dir_all(p);
    }
}
EOF_RM
flagged=()
n=0
while IFS= read -r l; do
  n=$((n + 1))
  [[ $l != *'// flagged'* ]] || flagged+=("$n")
done <"$tmp/rm.rs"
rms="$tmp/rms"
for f in clean.rs build_output.rs run/phase_run.rs other.rs run/tamper.rs; do
  mkdir -p "$(dirname "$rms/cli/src/$f")"
  cp "$tmp/rm.rs" "$rms/cli/src/$f"
done
out=$(cd "$rms" && "$pack/check.sh" ast-grep cli 2>"$tmp/stderr") || true
got=$(jq -rs 'map(select(.rule == "tree-removal-skips-modes")) | sort_by(.file, .line)
  | map("\(.file) \(.line)") | join(" ")' <<<"$out" 2>/dev/null) || true
want=
for f in build_output.rs clean.rs run/phase_run.rs; do
  for l in "${flagged[@]}"; do want+="${want:+ }cli/src/$f $l"; done
done
[ "$got" = "$want" ] || {
  printf 'FAIL tree-removal-skips-modes: findings %q, want %q, stderr:\n%s\n' "$got" "$want" "$(<"$tmp/stderr")" >&2
  failed=1
}

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
echo "rules/rust: ratchet-binary-size.sh, ratchet.toml, harness-kind-construction and tree-removal-skips-modes pass"
