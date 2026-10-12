#!/usr/bin/env bash
# Run what .github/workflows/ci.yml runs, in order, stopping at the first failure.
#
# The .githooks pre-commit and pre-push hooks run it.
#
# Uses the ns this run builds: cli/target/debug/ns, or under $CARGO_TARGET_DIR when that
# is set, as the pre-push hook sets it. The private guard reads
# PRIVATE_DENYLIST from the environment, as in CI. It fails first on a missing or
# wrong-version tool from scripts/ci-tools.toml, which CI installs.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [ "$#" -ne 0 ]; then
  echo "usage: scripts/ci-local.sh" >&2
  exit 2
fi

ns=${CARGO_TARGET_DIR:-cli/target}/debug/ns

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
  (cd evals/fixtures/py-inventory && pytest -q)
}

rule_pack_tests() {
  "rules/$1/tests/run.sh" || return 1
  scripts/check-git-dir-decoy.sh "rules/$1/tests/run.sh"
}

brief_checker_tests() {
  command -v python3 >/dev/null || { echo "ci-local: python3 is required for the brief checker tests" >&2; return 1; }
  local test
  for test in skills/ns-troubleshoot/evals/cases/*/test_check_brief.py; do
    (cd "$(dirname "$test")" && python3 -m unittest -q test_check_brief) || return 1
  done
}

step "pinned tools (scripts/ci-tools.toml)" scripts/ci-tools.sh check
step "cargo fmt --check" cargo fmt --manifest-path cli/Cargo.toml --check
step "cargo clippy" cargo clippy --manifest-path cli/Cargo.toml --all-targets -- -D warnings
step "cargo test" cargo test --manifest-path cli/Cargo.toml
step "cargo test under a decoy GIT_DIR" scripts/check-git-dir-decoy.sh
step "build ns" cargo build --manifest-path cli/Cargo.toml
step "ns lint" "$ns" lint skills --human
step "ns lint .agents/skills" "$ns" lint .agents/skills --human
step "ns check-markers" "$ns" check-markers --human
step "ns rules check" "$ns" rules check --human
step "ns eval --dry-run" eval_dry_run
step "py-inventory fixture tests" fixture_tests
step "Python rule pack tests" rule_pack_tests python
step "C rule pack tests" python3 -I -m unittest discover -s rules/c/tests
step "Java rule pack tests" rule_pack_tests java
step "ns-troubleshoot brief checker tests" brief_checker_tests
step "private guard" scripts/check-private.sh

echo "ci-local: all checks passed" >&2
