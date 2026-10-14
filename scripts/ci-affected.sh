#!/usr/bin/env bash
# The tests for the paths in NS_GATE_FILES, one per line. Under [gate] local = "build-only",
# ns run's gate after review and in the merge step runs this after ci-local.sh --fast, with the
# paths changed since its full gate last went green (docs/factory/run.md, "Gate").
#
# cargo test runs only the test binaries that read a changed path, as scripts/ci-affected.map
# lists them: all of it takes minutes, and most changes outside cli/ are read by a few binaries
# or none. A path the map sends to `all` (cli/, the scripts that run the tests) runs all of it.
# The ast-grep commands of the rules row run on any change, against the committed baseline:
# they take seconds, and a review fix that breaks one would otherwise first fail in GitHub CI.
# Clippy and cargo-deny stay in the full gate. A rule pack's tests (each pack
# scripts/rule-packs.sh lists) and the py-inventory fixture tests run when a path under them
# changed. With NS_GATE_FILES unset, or holding a path that is not a plain repo path, every
# step runs; set and empty, none does.
#
# --cargo-test runs only the cargo test step. --changed <prefix> exits 0 when a path under
# <prefix> changed, or every step would run, and 1 otherwise. GitHub CI runs both on a pull
# request, with the PR's changed paths.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

usage() {
  echo "usage: scripts/ci-affected.sh [--cargo-test | --changed <prefix>]" >&2
  exit 2
}

mode=all
case $# in
  0) ;;
  1) [ "$1" = --cargo-test ] || usage; mode=cargo-test ;;
  2) [ "$1" = --changed ] || usage; mode=changed ;;
  *) usage ;;
esac

files=${NS_GATE_FILES-all}
while IFS= read -r f; do
  [ -n "$f" ] || continue
  case /$f/ in
    //* | */../* | */./* | *'"'* | *[[:cntrl:]]*) files=all ;;
  esac
done <<<"$([ "$files" = all ] || printf '%s' "$files")"

changed() {
  [ "$files" = all ] || grep -q "^$1" <<<"$files"
}

if [ "$mode" = changed ]; then
  changed "$2"
  exit
fi

if [ -z "$files" ]; then
  echo "ci-affected: no files changed" >&2
  exit 0
fi

# The cargo test target flags for the changed paths, one per line, or `all`.
targets() {
  if [ "$files" = all ]; then
    echo all
    return
  fi
  local pattern words f i w out=
  while read -r pattern words; do
    while IFS= read -r f; do
      [[ $f == $pattern ]] || continue
      if [ "$words" = all ]; then
        echo all
        return
      fi
      read -ra w <<<"$words"
      for ((i = 0; i < ${#w[@]}; i += 2)); do
        out+="${w[i]} ${w[i + 1]}"$'\n'
      done
      break
    done <<<"$files"
  done <scripts/ci-affected.map
  printf '%s' "$out" | LC_ALL=C sort -u
}

step() {
  local name=$1
  shift
  echo "==> $name" >&2
  if ! "$@"; then
    echo "ci-affected: FAILED at step: $name" >&2
    exit 1
  fi
}

packs=$(scripts/rule-packs.sh)
selected=$(targets)
if [ "$selected" = all ]; then
  step "cargo test" cargo test --manifest-path cli/Cargo.toml
elif [ -n "$selected" ]; then
  read -ra args <<<"$(tr '\n' ' ' <<<"$selected")"
  step "cargo test ${args[*]}" cargo test --manifest-path cli/Cargo.toml "${args[@]}"
else
  echo "ci-affected: no test binary reads these paths" >&2
fi
if [ "$mode" = cargo-test ]; then
  exit 0
fi

# ci-local.sh --fast, which runs first, built this ns.
step "ns rules check --only ast-grep" "${CARGO_TARGET_DIR:-cli/target}/debug/ns" rules check --only ast-grep --human
# On fd 3, so a pack's tests can't read the list from stdin.
while IFS=$'\t' read -r -u 3 pack name; do
  if [ -n "$pack" ] && changed "rules/$pack/"; then
    step "$name rule pack tests" "rules/$pack/tests/run.sh"
  fi
done 3<<<"$packs"
if changed evals/fixtures/py-inventory/; then
  step "py-inventory fixture tests" bash -c 'cd evals/fixtures/py-inventory && pytest -q'
fi
echo "ci-affected: all checks passed" >&2
