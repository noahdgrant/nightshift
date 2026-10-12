#!/bin/sh
# The Java rule pack: run one check over a directory and print each finding as the JSON line
# `ns rules check` reads ({tool, rule, file, line, message[, snippet]}). Run it from the repo root.
#
#   check.sh pmd|ast-grep|size <dir>
#
# It checks the .java files under <dir>: in a git work tree the tracked and untracked ones git
# doesn't ignore, else every one outside a dot directory. Exits 0 with no findings, 1 with
# findings, and 2 with `check.sh:` lines on stderr, and no findings, when the check could not run.
set -eu

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

MAX_LINES=1000

usage() {
  echo "usage: $0 pmd|ast-grep|size <dir>" >&2
  exit 2
}

broken() {
  printf '%s\n' "$@" | sed 's/^/check.sh: /' >&2
  exit 2
}

[ $# -eq 2 ] || usage
check=$1
dir=${2%/}
[ -n "$dir" ] || dir=/
pack=$(cd "$(dirname "$0")" && pwd)

need() {
  for t in "$@"; do
    case $t in
      jq) how="your package manager (apt install jq)" ;;
      java) how="a JDK or JRE, 11 or later (apt install openjdk-21-jre-headless)" ;;
      ast-grep) how="uv tool install ast-grep-cli" ;;
    esac
    command -v "$t" >/dev/null 2>&1 || broken "$t is not on PATH; install it with $how"
  done
}

case $check in
  pmd)
    need jq java
    [ -x "$pack/pmd/bin/pmd" ] || broken "PMD is not installed in $pack/pmd; rerun the pack's install.sh"
    ;;
  ast-grep) need jq ast-grep ;;
  size) need jq ;;
  *) usage ;;
esac
[ -d "$dir" ] || broken "$dir is not a directory"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
files=$tmp/files
report=$tmp/report
log=$tmp/log
status=0

if git -C "$dir" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git -C "$dir" ls-files -z -co --exclude-standard -- '*.java' | tr '\0' '\n' |
    DIR="$dir" awk '{ print (ENVIRON["DIR"] == "." ? "" : ENVIRON["DIR"] "/") $0 }' >"$tmp/listed"
else
  find "$dir" -mindepth 1 -name '.*' -prune -o -name '*.java' -print | sed 's|^\./||' >"$tmp/listed"
fi
while IFS= read -r file; do
  [ ! -f "$file" ] || printf '%s\n' "$file"
done <"$tmp/listed" >"$files"
[ -s "$files" ] || exit 0

case $check in
  pmd)
    "$pack/pmd/bin/pmd" check --no-cache --no-progress -R "$pack/ruleset.xml" -f json \
      --file-list "$files" -r "$report" >"$log" 2>&1 || status=$?
    # 4 is PMD's exit status when it found violations and nothing went wrong.
    if [ "$status" -ne 0 ] && [ "$status" -ne 4 ]; then
      errors=$(jq -r '(.processingErrors + .configurationErrors)[]
        | "\(.filename // "PMD"): \(.message | split("\n")[0])"' "$report" 2>/dev/null || true)
      broken "PMD exited $status, so its report is incomplete:" "${errors:-$(tail -n 20 "$log")}"
    fi
    jq -c '.files[] | .filename as $file | .violations[]
      | {tool: "pmd", rule: .rule, file: $file, line: .beginline,
         message: "\(.description) (\(.externalInfoUrl))"}' "$report" >"$tmp/found"
    ;;
  ast-grep)
    tr '\n' '\0' <"$files" | xargs -0 ast-grep scan -c "$pack/sgconfig.yml" --json=stream -- \
      >"$report" 2>"$log" || status=$?
    [ "$status" -eq 0 ] && [ ! -s "$log" ] || broken "ast-grep exited $status:" "$(cat "$log")"
    # A @SuppressWarnings with a comment after it on its line has its reason.
    jq -c '. as $m | select((.text | startswith("@SuppressWarnings") or startswith("@java.lang.SuppressWarnings"))
          and (.lines | split("\n") | last | .[$m.range.end.column:] | test("^\\s*(//|/\\*)"))
          | not)
      | {tool: "ast-grep", rule: .ruleId, file: .file, line: (.range.start.line + 1),
         message: .message}' "$report" >"$tmp/found"
    ;;
  size)
    while IFS= read -r file; do
      last=$(awk '/[^[:space:]]/ { last = NR } END { print last + 0 }' <"$file")
      [ "$last" -le "$MAX_LINES" ] || jq -cn --arg file "$file" --argjson line "$last" --argjson max "$MAX_LINES" \
        '{tool: "check.sh", rule: "file-size", file: $file, line: $line,
          message: "the file is past \($max) lines; split it along a seam",
          snippet: "file past \($max) lines"}'
    done <"$files" >"$tmp/found"
    ;;
esac

[ -s "$tmp/found" ] || exit 0
cat "$tmp/found"
exit 1
