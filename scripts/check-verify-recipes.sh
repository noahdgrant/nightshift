#!/usr/bin/env bash
# Check the recipes of the repo-local verify skill and run the ones that set up a sandbox and
# drive `ns run`, so a stale recipe fails CI instead of failing an agent (#362, #563).
#
#   scripts/check-verify-recipes.sh [SKILL.md]
#
# SKILL.md defaults to .agents/skills/verify-nightshift/SKILL.md; its feature files are in
# features/ beside it. A recipe is a fenced bash block under SKILL.md's `## Doctor` or `## Drive`,
# or under a feature file's `## Driving it with ns`. Prose and blocks under other headings are not
# recipes. First, without running anything, every line of every recipe must start with
# `"$V/sandbox" `, chain, redirect or substitute nothing outside single quotes (`;` `&` `|` `<` `>`,
# a backtick, `$(`), use no glob (`*` `?`), bracket, brace, `$'`, `$"` or `..` outside single
# quotes, close every quote and end no line in a backslash, and name a `.ns/` path only as one
# unit's folder (`.ns/$NS_UNIT`, `.ns/<unit-id>` or a concrete id). Every fence under those
# headings must be exactly ```bash. Fails naming the file, heading and block of each line that does not.
#
# Then runs, in order in one bash shell (errexit, nounset, pipefail) from the repo root, with stdin
# from /dev/null, git location variables unset, OUT in a temporary directory and NS_UNIT, unexported,
# set to 7-fix-the-thing (the unit run.md drives) whatever the caller's is: the blocks under
# SKILL.md's `## Launch`, `## Doctor` and `## Drive`, then those under features/run.md's
# `## Driving it with ns`. This script holds no copy of them. Launch builds ns itself: under
# $CARGO_TARGET_DIR when set, else cli/target.
#
# Fails, naming the block, when a block exits non-zero or a heading has no bash block. Fails when
# Doctor prints a `NOT FAKE` line, or when its last JSON line does not show ok true, no problems,
# forge account octocat, no ssh probe of the forge (a real host) and claude at $V/bin/claude.
# Fails when $OUT/run-pass.out, run.md's `ns run --issue 7`, is not JSON whose last value has
# outcome done.
# Removes the sandbox ($V) and the evidence directory on exit.
set -euo pipefail

skill=${1:-.agents/skills/verify-nightshift/SKILL.md}
if [ ! -f "$skill" ]; then
  echo "check-verify-recipes: no skill file: $skill" >&2
  exit 1
fi
skill=$(realpath -- "$skill")
features=$(dirname "$skill")/features

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# An inherited location (from a hook or a decoy) would send the recipes' git commands to it.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Writes the bash blocks under `## <heading>` of <file> to <base>.<n>.sh, each line without its
# indent, and prints their count: 0 when <file> does not exist. Writes to <base>.bad a line for
# each fence under the heading, counted from 1, whose info string is not exactly `bash`.
extract() {
  [ -f "$1" ] || { echo 0; return; }
  : > "$3.bad"
  awk -v h="## $2" -v base="$3" '
    fence && /^ *(```|~~~)/ { fence = 0; next }
    fence { if (keep) { sub(/^ */, ""); print > file }; next }
    /^ *(```|~~~)/ {
      fence = 1
      keep = sec && $0 ~ /^ *```bash$/
      if (keep) file = base "." ++n ".sh"
      if (sec) { f++; if (!keep) { l = $0; sub(/^ */, "", l); print f ": " l > (base ".bad") } }
      next
    }
    /^## / { sec = ($0 == h); f = 0 }
    END { print n + 0 }
  ' "$1"
}

# Prints <line> three times: without anything quoted, without single-quoted text only, and a word
# for each problem the other two cannot show: `ansi` for a `$'` or `$"` outside single quotes,
# `open` for a quote still open or a backslash last at the end of the line. A single quote, a
# double quote and a backslash are read as the shell reads them.
unquoted() {
  awk '
    { bare = ""; dq = ""; q = ""; flags = ""; n = length($0)
      for (i = 1; i <= n; i++) {
        c = substr($0, i, 1)
        if (q == "\x27") { if (c == q) q = ""; continue }
        if (c == "$" && substr($0, i + 1, 1) ~ /^[\x27"]$/) flags = flags " ansi"
        if (c == "\\") { if (i == n) flags = flags " open"; i++; if (q == "\"") dq = dq c substr($0, i, 1); continue }
        if (q == "\"") { if (c == q) q = ""; else dq = dq c; continue }
        if (c == "\x27" || c == "\"") { q = c; continue }
        bare = bare c; dq = dq c
      }
      if (q != "") flags = flags " open"
      print bare; print dq; print flags }
  ' <<<"$1"
}

# Prints a line for each recipe line in the blocks under `## <heading>` of <file> that does not
# run in the sandbox, chains or substitutes a command, uses a glob, brace, `$'`, `$"` or `..`, ends
# a line inside a quote or after a backslash, or names a .ns/ path other than one unit's folder;
# and for each fence there that is not bash.
lint() {
  local file=$1 heading=$2 n i where line bare dq flags tok
  n=$(extract "$file" "$heading" "$tmp/lint")
  while IFS= read -r line; do
    [ -z "$line" ] || echo "$(basename "$file") ## $heading block ${line%%:*} is not a bash block: ${line#*: }"
  done < "$tmp/lint.bad"
  for i in $(seq "$n"); do
    where="$(basename "$file") ## $heading block $i"
    while IFS= read -r line; do
      case "$line" in '' | '#'*) continue ;; esac
      case "$line" in '"$V/sandbox" '*) ;; *) echo "$where does not run in the sandbox: $line" ;; esac
      { read -r bare; read -r dq; read -r flags; } < <(unquoted "$line")
      case "$dq" in *[*?]*) echo "$where uses a glob: $line" ;; esac
      case "$flags" in *ansi*) echo "$where uses \$' or \$\" quoting: $line" ;; esac
      case "$flags" in *open*) echo "$where ends a line inside a quote or after a backslash: $line" ;; esac
      if [[ "$bare" == *[\;\&\|\<\>]* || "$dq" == *'`'* || "$dq" == *'$('* ]]; then
        echo "$where runs something outside the sandbox (chain, redirect or substitution): $line"
      fi
      if [[ "$bare" == *[\[\{]* || "$dq" == *..* ]]; then
        echo "$where uses a bracket, brace or ..: $line"
      fi
      for tok in $(grep -o '[^ ]*\.ns/[^ ]*' <<<"$line" || true); do
        if ! grep -Eq '\.ns/(\$NS_UNIT|<unit-id>|[0-9]+-[a-z0-9-]+)(/[A-Za-z0-9_-][A-Za-z0-9_.-]*)*["'\'']?$' <<<"$tok" || [[ "$tok" == *..* ]]; then
          echo "$where names a .ns/ path other than one unit's folder (.ns/\$NS_UNIT): $line"
        fi
      done
    done < "$tmp/lint.$i.sh"
  done
}

found=$(
  lint "$skill" Doctor
  lint "$skill" Drive
  for f in "$features"/*.md; do
    [ -f "$f" ] || continue
    [ "$(basename "$f")" = README.md ] && continue
    if ! grep -qx '## Driving it with ns' "$f"; then
      echo "$(basename "$f") has no ## Driving it with ns heading, so its recipes would go unlinted"
    fi
    lint "$f" "Driving it with ns"
  done
)
if [ -n "$found" ]; then
  sed 's/^/check-verify-recipes: /' <<<"$found" >&2
  exit 1
fi

# Writes the blocks under `## <heading>` of <file> to the run script, each named <name> block <n>.
emit() {
  local file=$1 heading=$2 name=$3 n i
  n=$(extract "$file" "$heading" "$tmp/$name")
  if [ "$n" -eq 0 ]; then
    echo "check-verify-recipes: no bash block under ## $heading in $file" >&2
    exit 1
  fi
  for i in $(seq "$n"); do
    printf 'block=%q\n' "$name block $i"
    cat "$tmp/$name.$i.sh"
  done
}

{
  printf 'out=%q\nexport OUT=%q\n' "$tmp/recipes.out" "$tmp/evidence"
  cat <<'EOF'
set -euo pipefail
unset V NS_UNIT
NS_UNIT=7-fix-the-thing
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
  emit "$skill" Launch Launch
  emit "$skill" Doctor Doctor
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
exec >"$out"
EOF
  emit "$skill" Drive Drive
  emit "$features/run.md" "Driving it with ns" run.md
  cat <<'EOF'
exec >&3
cat "$out"
: > "$out"
block="run checks"
outcome=$(jq -c -s '.[-1].outcome' "$OUT/run-pass.out" 2>/dev/null) || outcome=
if [ -z "$outcome" ]; then
  echo "check-verify-recipes: run: no run-pass result in \$OUT/run-pass.out" >&2
  cat "$OUT/run-pass.err" >&2 2>/dev/null || true
  exit 1
fi
if [ "$outcome" != '"done"' ]; then
  echo "check-verify-recipes: run: run-pass outcome is $outcome, not \"done\"" >&2
  cat "$OUT/run-pass.err" >&2 2>/dev/null || true
  exit 1
fi
EOF
} > "$tmp/run.sh"

bash "$tmp/run.sh" < /dev/null
