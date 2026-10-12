#!/usr/bin/env bash
# The tests for the paths in NS_GATE_FILES, one per line. Under [gate] local = "build-only",
# ns run's gate after review and in the merge step runs this after ci-local.sh --fast, with the
# paths changed since its full gate last went green (docs/FACTORY.md, "Gate").
#
# cargo test runs on any change, since the cli tests read files across the repo. A rule pack's
# tests and the py-inventory fixture tests run when a path under them changed. With
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

step "cargo test" cargo test --manifest-path cli/Cargo.toml
for pack in python js java; do
  if changed "rules/$pack/"; then
    step "$pack rule pack tests" "rules/$pack/tests/run.sh"
  fi
done
if changed rules/c/; then
  step "C rule pack tests" python3 -I -m unittest discover -s rules/c/tests
fi
if changed evals/fixtures/py-inventory/; then
  step "py-inventory fixture tests" bash -c 'cd evals/fixtures/py-inventory && pytest -q'
fi
echo "ci-affected: all checks passed" >&2
