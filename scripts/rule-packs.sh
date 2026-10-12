#!/usr/bin/env bash
# The rule packs CI tests: each rules/<pack>/ with a tests/run.sh, one line each as
# `<pack><TAB><name>`, in order of <pack>. The name is the one line of rules/<pack>/NAME, and
# the pack's CI step is "<name> rule pack tests". scripts/ci-local.sh, scripts/ci-affected.sh
# and the rule-packs job in .github/workflows/ci.yml all read this list, so a new pack needs no
# change outside its own directory.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
for run in rules/*/tests/run.sh; do
  [ -f "$run" ] || continue
  pack=${run#rules/}
  pack=${pack%%/*}
  name=
  [ ! -f "rules/$pack/NAME" ] || name=$(<"rules/$pack/NAME")
  if [ -z "$name" ] || [[ $name == *$'\n'* ]]; then
    echo "rule-packs: rules/$pack/NAME must hold the pack's name on one line" >&2
    exit 1
  fi
  printf '%s\t%s\n' "$pack" "$name"
done
