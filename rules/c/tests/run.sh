#!/usr/bin/env bash
# Run the C pack's tests, test_rules_c.py, from any directory.
set -euo pipefail

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

exec python3 -I -m unittest discover -s "$(dirname "${BASH_SOURCE[0]}")"
