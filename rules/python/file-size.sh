#!/bin/sh
# A file whose last non-blank line is past line 1000 is hard for an agent to hold; split it
# along its seams. Blank lines at the top count, which an ast-grep rule can't see.
# Prints `file:line:1: error[file-size]: ...` for each such .py file under the paths
# given (default: the current directory), the format `ns rules check` reads.
#
#   file-size.sh [path...]
set -eu

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

[ "$#" -gt 0 ] || set -- .

for path in "$@"; do
  if [ -f "$path" ]; then
    printf '%s\n' "$path"
  elif git -C "$path" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$path" ls-files -z -co --exclude-standard -- '*.py' | tr '\0' '\n' | PREFIX="${path%/}/" awk '{ print ENVIRON["PREFIX"] $0 }'
  else
    find "$path" -mindepth 1 -name '.*' -prune -o -name '*.py' -print
  fi
done | while IFS= read -r file; do
  [ -f "$file" ] || continue
  FILE=$file awk '
    /[^[:space:]]/ { last = NR }
    END {
      if (last > 1000)
        printf "%s:%d:1: error[file-size]: file is past 1000 lines; split it\n", ENVIRON["FILE"], last
    }' <"$file"
done
