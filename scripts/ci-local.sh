#!/bin/sh
# Runs CI locally: the fmt, clippy and test checks .github/workflows/ci.yml runs.
# CI also runs the marker check through `ns check`. This script cannot call
# `ns check`: that runs .nightshift/check, which is this script, so it would
# run itself. --fast skips the tests.
set -eu
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX
cd "$(dirname "$0")/../cli"
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
[ "${1-}" = --fast ] || cargo test --locked
