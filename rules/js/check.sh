#!/bin/sh
# The JavaScript/TypeScript rule pack: run one tool over a directory and print each finding as
# the JSON line `ns rules check` reads ({tool, rule, file, line, message}). Run it from the repo
# root. The tools come from the pack's own node_modules (install.sh runs `npm ci` there), except
# ast-grep, which must be on PATH.
#
#   check.sh eslint|tsc|ast-grep <dir>
#
# Exits with the tool's status. When the tool fails and nothing it printed is a finding, its
# output goes to stderr instead, so `ns rules check` reports the failure. tsc runs only when
# <dir> has a tsconfig.json, and otherwise prints nothing and exits 0; an error in the
# tsconfig.json exits 2 with no findings.
set -eu

usage() {
  echo "usage: $0 eslint|tsc|ast-grep <dir>" >&2
  exit 2
}

[ $# -eq 2 ] || usage
tool=$1
dir=${2%/}
pack=$(cd "$(dirname "$0")" && pwd)
bin=$pack/node_modules/.bin

need() {
  for t in "$@"; do
    case $t in
      jq) how="your package manager (apt install jq)" ;;
      node) how="your package manager (apt install nodejs npm)" ;;
      ast-grep) how="uv tool install ast-grep-cli" ;;
    esac
    command -v "$t" >/dev/null 2>&1 || {
      echo "check.sh: $t is not on PATH; install it with $how" >&2
      exit 127
    }
  done
}

case $tool in
  eslint | tsc)
    need node jq
    [ -x "$bin/$tool" ] || {
      echo "check.sh: $bin/$tool is missing; install the pack's tools with: (cd $pack && npm ci)" >&2
      exit 127
    }
    ;;
  ast-grep) need ast-grep jq ;;
  *) usage ;;
esac
[ -d "$dir" ] || {
  echo "check.sh: $dir is not a directory" >&2
  usage
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
report=$tmp/report
log=$tmp/log
status=0

case $tool in
  eslint)
    # ESLint runs from <dir>, so the config's globs and its tsconfig.json test are relative to
    # it, and -c keeps the repo's own ESLint config out.
    (cd "$dir" && "$bin/eslint" -c "$pack/eslint.config.mjs" --no-error-on-unmatched-pattern \
      -f json .) >"$report" 2>"$log" || status=$?
    # A message with no rule is a parse error (fatal) or an unused disable directive.
    found=$(jq -c --arg dir "$dir" --arg abs "$(cd "$dir" && pwd -P)" '.[] | .filePath as $f
      | .messages[]
      | {tool: "eslint",
         rule: (.ruleId // if .fatal then "parse-error" else "unused-disable-directive" end),
         file: ($dir + "/" + ($f | ltrimstr($abs + "/"))), line: .line, message: .message}' \
      "$report" 2>/dev/null || true)
    ;;
  tsc)
    [ -f "$dir/tsconfig.json" ] || exit 0
    (cd "$dir" && "$bin/tsc" --noEmit --pretty false -p tsconfig.json) >"$report" 2>"$log" ||
      status=$?
    # A config error, in tsconfig.json or with no file, means tsc may have checked nothing, so
    # it fails the run rather than become a finding a baseline could hide.
    if grep -E '^error TS|^[^ ]*\.json\([0-9]+,[0-9]+\): error TS' "$report" >"$log"; then
      sed 's/^/check.sh: /' "$log" >&2
      echo "check.sh: $dir/tsconfig.json has errors, so tsc checked nothing; fix them and rerun" >&2
      exit 2
    fi
    # One line per error, `file(line,col): error TSnnnn: message`; the indented lines after
    # one add detail to it.
    found=$(jq -cR --arg dir "$dir" 'capture("^(?<file>[^ ].*)\\((?<line>[0-9]+),[0-9]+\\): error (?<rule>TS[0-9]+): (?<message>.*)$")
      | {tool: "tsc", rule: .rule, file: ($dir + "/" + .file), line: (.line | tonumber),
         message: .message}' "$report")
    ;;
  ast-grep)
    # Dot directories are scanned, as ESLint does; .git, node_modules and the installed pack
    # are not.
    ast-grep scan -c "$pack/sgconfig.yml" --no-ignore hidden --globs '!.git' \
      --globs '!node_modules' --globs '!.nightshift/rules' --json=stream "$dir" \
      >"$report" 2>"$log" || status=$?
    found=$(jq -cR 'fromjson? | {tool: "ast-grep", rule: .ruleId, file: .file,
      line: (.range.start.line + 1), message: .message}' "$report")
    ;;
esac

if [ -n "$found" ]; then
  printf '%s\n' "$found"
elif [ "$status" -ne 0 ]; then
  cat "$report" "$log" >&2
fi
exit "$status"
