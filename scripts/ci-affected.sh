#!/usr/bin/env bash
# The tests for the paths in NS_GATE_FILES, one per line. Under [gate] local = "build-only",
# ns run's gate after review and in the merge step runs this after ci-local.sh --fast, with the
# paths changed since its full gate last went green (docs/FACTORY.md, "Gate").
#
# cargo test runs on any change, since the cli tests read files across the repo. A rule pack's
# tests (each pack scripts/rule-packs.sh lists) and the py-inventory fixture tests run when a
# path under them changed. With
# NS_GATE_FILES unset every step runs; set and empty, none does.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

files=${NS_GATE_FILES-all}
if [ -z "$files" ]; then
  echo "ci-affected: no files changed" >&2
  exit 0
fi

changed() {
  [ "$files" = all ] || grep -q "^$1" <<<"$files"
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
step "cargo test" cargo test --manifest-path cli/Cargo.toml
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
