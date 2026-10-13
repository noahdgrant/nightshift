#!/usr/bin/env bash
# Run the Launch and Doctor recipes of the repo-local verify skill, so a stale recipe fails CI
# instead of failing an agent (#362).
#
#   scripts/check-verify-recipes.sh [SKILL.md]
#
# SKILL.md defaults to .agents/skills/verify-nightshift/SKILL.md. The recipes are the fenced
# bash blocks under its `## Launch` and `## Doctor` headings; this script holds no copy of them.
# They run in order in one bash shell (errexit, nounset, pipefail) from the repo root, with
# stdin from /dev/null, git location variables unset and OUT in a temporary directory. The
# recipe builds ns itself: under $CARGO_TARGET_DIR when set, else cli/target.
#
# Fails, naming the block, when a block exits non-zero or a heading has no bash block. Fails
# when the recipes print a `NOT FAKE` line, or when their last JSON line (Doctor's) does not show ok true, no
# problems, forge account octocat, no ssh probe of the forge (a real host) and claude at
# $V/bin/claude. Removes the sandbox ($V) and the evidence directory on exit.
set -euo pipefail

skill=${1:-.agents/skills/verify-nightshift/SKILL.md}
if [ ! -f "$skill" ]; then
  echo "check-verify-recipes: no skill file: $skill" >&2
  exit 1
fi
skill=$(realpath -- "$skill")

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# An inherited location (from a hook or a decoy) would send the recipes' git commands to it.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Writes the bash blocks under `## <heading>` to $tmp/<heading>.<n>.sh and prints their count.
extract() {
  awk -v h="## $1" -v base="$tmp/$1" '
    fence && /^```/ { fence = 0; next }
    fence { if (keep) print > file; next }
    /^```/ { fence = 1; keep = sec && $0 == "```bash"; if (keep) file = base "." ++n ".sh"; next }
    /^## / { sec = ($0 == h) }
    END { print n + 0 }
  ' "$skill"
}

{
  printf 'out=%q\nexport OUT=%q\n' "$tmp/recipes.out" "$tmp/evidence"
  cat <<'EOF'
set -euo pipefail
unset V
block=setup
exec 3>&1 >"$out"
finish() {
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    exec >&3
    cat "$out" >&2
    echo "check-verify-recipes: FAILED in $block (exit $rc)" >&2
  fi
  if [ -n "${V-}" ] && [ -d "$V" ]; then rm -rf -- "$V"; fi
}
trap finish EXIT
trap 'exit 143' TERM INT HUP
EOF
  for heading in Launch Doctor; do
    n=$(extract "$heading")
    if [ "$n" -eq 0 ]; then
      echo "check-verify-recipes: no bash block under ## $heading in $skill" >&2
      exit 1
    fi
    for i in $(seq "$n"); do
      printf 'block=%q\n' "$heading block $i"
      cat "$tmp/$heading.$i.sh"
    done
  done
  cat <<'EOF'
exec >&3
cat "$out"
printed=$(cat "$out")
: > "$out"
block="Doctor checks"
found=$(grep '^NOT FAKE' <<<"$printed" || true)
json=$(grep '^{' <<<"$printed" | tail -n 1 || true)
if [ -z "$json" ]; then
  found+=${found:+$'\n'}"printed no JSON line"
else
  found+=${found:+$'\n'}$(jq -r --arg claude "$V/bin/claude" '
    (if .ok != true then "ok is \(.ok | tojson), not true" else empty end),
    (if .problems != [] then "problems is \(.problems | tojson), not []" else empty end),
    (if .forge.account != "octocat" then "forge.account is \(.forge.account | tojson), not \"octocat\"" else empty end),
    (if .forge.ssh != null then "forge.ssh is \(.forge.ssh | tojson), so doctor probed a real ssh host" else empty end),
    (if .claude != $claude then "claude is \(.claude | tojson), not \($claude | tojson)" else empty end)
  ' <<<"$json")
fi
found=$(sed '/^$/d; s/^/check-verify-recipes: Doctor: /' <<<"$found")
[ -z "$found" ] || { echo "$found" >&2; exit 1; }
EOF
} > "$tmp/run.sh"

bash "$tmp/run.sh" < /dev/null
