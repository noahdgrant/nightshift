#!/usr/bin/env bash
# Run what .github/workflows/ci.yml runs, in order, stopping at the first failure.
#
#   scripts/ci-local.sh          every check
#   scripts/ci-local.sh --fast   fmt, clippy, ns lint and the private guard (the pre-commit hook)
#
# Uses this worktree's own cli/target/debug/ns. The private guard reads
# PRIVATE_DENYLIST from the environment, as in CI.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fast=0
case "${1:-}" in
  "") ;;
  --fast) fast=1 ;;
  *) echo "usage: scripts/ci-local.sh [--fast]" >&2; exit 2 ;;
esac

ns=cli/target/debug/ns

step() {
  local name=$1
  shift
  echo "==> $name" >&2
  if ! "$@"; then
    echo "ci-local: FAILED at step: $name" >&2
    exit 1
  fi
}

eval_dry_run() {
  local tmp rc=0
  command -v python3 >/dev/null || { echo "ci-local: python3 is required for the eval plan check" >&2; return 1; }
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"; trap - RETURN' RETURN
  # The python capability is declared so py-* cases are planned, not skipped.
  # zephyr is left out: its cases skip, as in CI.
  printf '[eval]\nharness = "claude"\n\n[eval.capability.python]\n' > "$tmp/ns-config.toml"
  NS_CONFIG="$tmp/ns-config.toml" "$ns" eval --dry-run > "$tmp/plan.json" || rc=$?
  # Fail on any skip other than the missing zephyr capability (a missing
  # fixture or skill also shows up as a skip).
  [ "$rc" -ne 0 ] || python3 -I - "$tmp/plan.json" <<'PY' || rc=$?
import json, sys
plan = json.load(open(sys.argv[1]))
bad = [s for s in plan["skipped"]
       if not s["reason"].startswith("missing capability: zephyr")]
for s in bad:
    print(f"skipped: {s['skill']}/{s.get('case', '')}: {s['reason']}", file=sys.stderr)
sys.exit(1 if bad else 0)
PY
  return "$rc"
}

fixture_tests() {
  local py
  for py in python3 python; do
    if command -v "$py" >/dev/null && "$py" -I -c 'import pytest' 2>/dev/null; then
      (cd evals/fixtures/py-inventory && "$py" -m pytest -q)
      return
    fi
  done
  echo "SKIPPED: py-inventory fixture tests (no python with pytest; pip install pytest to run them)" >&2
}

brief_checker_tests() {
  command -v python3 >/dev/null || { echo "ci-local: python3 is required for the brief checker tests" >&2; return 1; }
  (cd skills/ns-troubleshoot/evals/cases/py-hold-expires-early && python3 -m unittest -q test_check_brief)
}

step "cargo fmt --check" cargo fmt --manifest-path cli/Cargo.toml --check
step "cargo clippy" cargo clippy --manifest-path cli/Cargo.toml --all-targets -- -D warnings
if [ "$fast" -eq 0 ]; then
  step "cargo test" cargo test --manifest-path cli/Cargo.toml
fi
step "build ns" cargo build --manifest-path cli/Cargo.toml
step "ns lint" "$ns" lint skills --human
if [ "$fast" -eq 0 ]; then
  step "ns eval --dry-run" eval_dry_run
  step "py-inventory fixture tests" fixture_tests
  step "ns-troubleshoot brief checker tests" brief_checker_tests
fi
step "private guard" scripts/check-private.sh

echo "ci-local: all checks passed" >&2
