#!/usr/bin/env bash
# Run the test suite with GIT_DIR and GIT_WORK_TREE aimed at a decoy repo, and fail if
# the suite touched it. Guards against a git command in the suite or the CLI that acts on
# an inherited GIT_DIR instead of the repo it names (#120).
#
#   scripts/check-git-dir-decoy.sh             runs cargo test --manifest-path cli/Cargo.toml
#   scripts/check-git-dir-decoy.sh <cmd>...    runs <cmd> instead
#
# Fails if the command fails, or if the decoy's .git/config or git for-each-ref output
# changed, or it became bare.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# An inherited location (from a hook, or this script run under another decoy) would send
# the setup below to the wrong repo.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

[ "$#" -gt 0 ] || set -- cargo test --manifest-path cli/Cargo.toml

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
decoy="$tmp/decoy"

g() { GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null git -C "$decoy" "$@"; }

git init -q "$decoy"
g -c user.name=decoy -c user.email=decoy@localhost commit -q --allow-empty -m decoy
g branch decoy-branch

snapshot() {
  cp "$decoy/.git/config" "$tmp/config.$1" && g for-each-ref > "$tmp/refs.$1"
}

snapshot before

rc=0
GIT_DIR="$decoy/.git" GIT_WORK_TREE="$decoy" "$@" || rc=$?

if ! snapshot after 2>/dev/null; then
  echo "check-git-dir-decoy: the decoy repo was removed or unreadable: $decoy" >&2
  [ "$rc" -eq 0 ] || echo "check-git-dir-decoy: command failed (exit $rc): $*" >&2
  exit 1
fi
changed=0
if ! diff -u "$tmp/config.before" "$tmp/config.after" >&2; then
  echo "check-git-dir-decoy: the decoy's .git/config changed" >&2
  changed=1
fi
if ! diff -u "$tmp/refs.before" "$tmp/refs.after" >&2; then
  echo "check-git-dir-decoy: the decoy's git for-each-ref output changed" >&2
  changed=1
fi
if [ "$(g config --bool core.bare)" = true ]; then
  echo "check-git-dir-decoy: the decoy became bare" >&2
  changed=1
fi

if [ "$rc" -ne 0 ]; then
  echo "check-git-dir-decoy: command failed (exit $rc): $*" >&2
  exit 1
fi
exit "$changed"
