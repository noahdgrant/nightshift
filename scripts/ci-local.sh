#!/usr/bin/env bash
# Run what .github/workflows/ci.yml runs, in order, stopping at the first failure.
#
# --fast runs only the quick steps: the pinned tools, fmt, clippy, the ns build, ns lint,
# ns check-markers, the ast-grep rules (ns rules check --only ast-grep) and the private guard.
# The .githooks pre-commit hook runs --fast on the working tree. The pre-push hook runs it at
# each commit pushed under [gate] local = "build-only", and the full suite otherwise.
#
# Uses the ns this run builds: cli/target/debug/ns, or under $CARGO_TARGET_DIR when that
# is set, as the pre-push hook sets it. The private guard reads
# PRIVATE_DENYLIST from the environment, as in CI. It fails first on a missing or
# wrong-version tool pinned in scripts/ci-tools.d/, which CI installs.
#
# A full run that passes is recorded in <git-common-dir>/ns/ci-passed/, keyed by the tree it
# tested and the hashes of the pins in scripts/ci-tools.d/ and this script (ADR-444). A later
# full run with the same key prints that it already passed and exits 0. The tree is the index's, and
# only when the working tree matches the index and holds no untracked file, so it is what the
# run tests: a commit's tree in a clean checkout, or a merge result staged but not committed.
# Otherwise the cache is not used. --no-cache runs anyway; --fast never uses the cache.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fast=0
use_cache=1
for arg in "$@"; do
  case "$arg:$fast:$use_cache" in
    --fast:0:*) fast=1 ;;
    --no-cache:*:1) use_cache=0 ;;
    *)
      echo "usage: scripts/ci-local.sh [--fast] [--no-cache]" >&2
      exit 2
      ;;
  esac
done

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

# Git on the checkout this script is in, never on a location a hook or a decoy exported.
cache_git() {
  env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE -u GIT_COMMON_DIR -u GIT_OBJECT_DIRECTORY \
    -u GIT_ALTERNATE_OBJECT_DIRECTORIES -u GIT_NAMESPACE -u GIT_PREFIX git "$@"
}

cache_max=${CI_LOCAL_CACHE_MAX:-200}
cache_dir=
cache_key=
cache_tree=

# Sets cache_dir, cache_key and cache_tree for what this run tests. Leaves cache_key empty
# outside a git checkout, or when the run would test more than the index's tree. With
# `after`, untracked files are allowed: the run itself may have left some.
compute_cache_key() {
  local top common tools script
  cache_key=
  top=$(cache_git rev-parse --show-toplevel 2>/dev/null) || return 0
  [ "$top" -ef . ] || return 0
  if ! cache_git diff --quiet 2>/dev/null; then
    echo "ci-local: the working tree differs from the index; not using the pass cache" >&2
    return 0
  fi
  if [ "${1-}" != after ] && [ -n "$(cache_git ls-files --others --exclude-standard 2>/dev/null)" ]; then
    echo "ci-local: the working tree has untracked files; not using the pass cache" >&2
    return 0
  fi
  cache_tree=$(cache_git write-tree 2>/dev/null) || return 0
  common=$(cache_git rev-parse --git-common-dir 2>/dev/null) || return 0
  common=$(cd "$common" && pwd) || return 0
  tools=$(sha256sum scripts/ci-tools.d/*.toml | sha256sum) || return 0
  script=$(sha256sum < scripts/ci-local.sh) || return 0
  cache_key=$(printf '%s %s %s\n' "$cache_tree" "${tools%% *}" "${script%% *}" | sha256sum) || return 0
  cache_key=${cache_key%% *}
  cache_dir=$common/ns/ci-passed
}

# Marks the entry $1 used last, so eviction never ranks it behind another entry: its time is now,
# or a second after the newest entry's when that is now or later (a tie, a coarse timestamp, a
# clock that stepped back). An entry another run evicts meanwhile is left out, and $1 is never
# created.
mark_used() {
  local now newest
  now=$(date +%s) || return 1
  newest=$({ stat -c %Y "$cache_dir"/* 2>/dev/null || true; } | sort -n | tail -n 1)
  [ "$now" -gt "${newest:-0}" ] || now=$((newest + 1))
  touch -c -d "@$now" "$1"
}

# Records a pass of the key computed before the run, if the checkout still holds that key, and
# keeps the cache_max entries used last.
record_pass() {
  local key=$cache_key entry tmp head old
  compute_cache_key after 2>/dev/null
  if [ "$cache_key" != "$key" ]; then
    echo "ci-local: the tree changed during the run; not recording the pass" >&2
    return 0
  fi
  mkdir -p "$cache_dir" || return 1
  entry=$cache_dir/$key
  head=$(cache_git rev-parse -q --verify --short HEAD 2>/dev/null) || head="no commit"
  tmp=$(mktemp "$cache_dir/.tmp.XXXXXX") || return 1
  if ! printf 'tree %s\nwhen %s\nwhere %s at %s\n' "$cache_tree" \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$PWD" "$head" > "$tmp" || ! mv -f "$tmp" "$entry"; then
    rm -f "$tmp"
    return 1
  fi
  mark_used "$entry" || return 1
  ls -1t "$cache_dir" | tail -n +$((cache_max + 1)) | while read -r old; do
    rm -f "$cache_dir/$old"
  done
}

# A step the --fast run leaves out.
full_step() {
  [ "$fast" -eq 1 ] || step "$@"
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

if [ "$fast" -eq 0 ]; then
  compute_cache_key
  if [ -n "$cache_key" ] && [ "$use_cache" -eq 1 ] && [ -f "$cache_dir/$cache_key" ]; then
    entry=$cache_dir/$cache_key
    when=$(sed -n 's/^when //p' "$entry")
    where=$(sed -n 's/^where //p' "$entry")
    mark_used "$entry" 2>/dev/null || true
    echo "ci-local: tree $cache_tree already passed ($when, $where)" >&2
    exit 0
  fi
fi

# The rule packs, `<pack><TAB><name>` a line, each a full step below. Listed in --fast runs too,
# so the pre-commit hook catches a pack with a bad NAME.
packs=$(scripts/rule-packs.sh) || exit 1

step "pinned tools (scripts/ci-tools.d)" scripts/ci-tools.sh check
step "cargo fmt --check" cargo fmt --manifest-path cli/Cargo.toml --check
step "cargo clippy" cargo clippy --manifest-path cli/Cargo.toml --all-targets -- -D warnings
full_step "cargo test" cargo test --manifest-path cli/Cargo.toml
full_step "cargo test under a decoy GIT_DIR" scripts/check-git-dir-decoy.sh
step "build ns" cargo build --manifest-path cli/Cargo.toml
step "ns lint" "$ns" lint skills --human
step "ns lint .agents/skills" "$ns" lint .agents/skills --human
full_step "verify-nightshift Launch and Doctor recipes" scripts/check-verify-recipes.sh
step "ns check-markers" "$ns" check-markers --human
full_step "ns rules check" "$ns" rules check --human
# The full step above covers it.
[ "$fast" -eq 0 ] || step "ns rules check --only ast-grep" "$ns" rules check --only ast-grep --human
full_step "ns ratchet check" "$ns" ratchet check --human
full_step "ns eval --dry-run" eval_dry_run
full_step "py-inventory fixture tests" fixture_tests
# On fd 3, so a pack's tests can't read the list from stdin.
while IFS=$'\t' read -r -u 3 pack name; do
  [ -z "$pack" ] || full_step "$name rule pack tests" rule_pack_tests "$pack"
done 3<<<"$packs"
full_step "ns-troubleshoot brief checker tests" brief_checker_tests
step "private guard" scripts/check-private.sh

echo "ci-local: all checks passed" >&2
if [ -n "$cache_key" ]; then
  record_pass || echo "ci-local: could not record the pass in $cache_dir" >&2
fi
