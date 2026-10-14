#!/usr/bin/env bash
# Sets NS_GATE_FILES in $GITHUB_ENV to the paths a pull request changes, from $BASE to $HEAD
# (`git diff --name-only BASE...HEAD`), so the later steps of a GitHub CI job run only the tests
# scripts/ci-affected.sh picks for them. A renamed file counts as its old path and its new one.
# When git can't list the paths (a shallow checkout without the base, an unknown commit), it
# warns and sets nothing, and every test runs.
set -euo pipefail

if ! files=$(git diff --no-renames --name-only "${BASE:?}...${HEAD:?}"); then
  echo "::warning::ci-pr-files: git could not list the paths changed from $BASE to $HEAD; every test runs"
  exit 0
fi
delim="ns-gate-files-$(date +%s%N)"
{
  echo "NS_GATE_FILES<<$delim"
  [ -z "$files" ] || printf '%s\n' "$files"
  echo "$delim"
} >>"${GITHUB_ENV:?}"
