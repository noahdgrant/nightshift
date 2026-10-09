#!/usr/bin/env bash
# Fail if a tracked file mentions a private reference.
#
#   PRIVATE_DENYLIST=foo,bar scripts/check-private.sh
#
# 1. Each comma-separated PRIVATE_DENYLIST term, matched case-insensitively as a
#    fixed string. Skipped when the list is empty. In CI the list comes from the
#    repo variable PRIVATE_DENYLIST, so no term is ever committed.
# 2. Absolute home paths (/home/<user>), outside cli/tests, whose fixtures may
#    use synthetic paths.
#
# Prints file:line:text for each hit and exits 1 if there are any.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

status=0

# git grep exits 0 on a hit, 1 on none, and >1 on an error, which must not pass.
grep_hits() {
  local rc=0
  git grep "$@" || rc=$?
  if [ "$rc" -gt 1 ]; then
    echo "error: git grep failed (exit $rc)" >&2
    exit 2
  fi
  return "$rc"
}

terms=()
IFS=',' read -r -a raw <<< "${PRIVATE_DENYLIST:-}"
for t in "${raw[@]}"; do
  t="${t#"${t%%[![:space:]]*}"}"   # trim leading whitespace
  t="${t%"${t##*[![:space:]]}"}"   # trim trailing whitespace
  [ -n "$t" ] && terms+=(-e "$t")
done

if [ "${#terms[@]}" -gt 0 ]; then
  if grep_hits -n -I -i -F "${terms[@]}" -- . ':(exclude).github/workflows/ci.yml'; then
    echo "error: tracked files match PRIVATE_DENYLIST (lines above)" >&2
    status=1
  fi
else
  echo "PRIVATE_DENYLIST is empty; checking home paths only" >&2
fi

if grep_hits -n -I -E '/home/[a-z]' -- . ':(exclude)cli/tests'; then
  echo "error: tracked files contain /home/<user> paths (lines above)" >&2
  status=1
fi

[ "$status" -eq 0 ] && echo "check-private: ok" >&2
exit "$status"
