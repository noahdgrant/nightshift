#!/bin/sh
# The Rust rule pack: run one tool over a crate and print each finding as the JSON line
# `ns rules check` reads ({tool, rule, file, line, message}). Run it from the repo root.
#
#   check.sh clippy|deny|ast-grep <crate-dir>
#
# Exits with the tool's status. When the tool fails and nothing it printed is a finding, its
# output goes to stderr instead, so `ns rules check` reports the failure.
set -eu

usage() {
  echo "usage: $0 clippy|deny|ast-grep <crate-dir>" >&2
  exit 2
}

[ $# -eq 2 ] || usage
tool=$1
dir=${2%/}

need() {
  for t in "$@"; do
    case $t in
      jq) how="your package manager (apt install jq)" ;;
      cargo) how="https://rustup.rs" ;;
      *) how="cargo install --locked $t" ;;
    esac
    command -v "$t" >/dev/null 2>&1 || {
      echo "check.sh: $t is not on PATH; install it with $how" >&2
      exit 127
    }
  done
}

case $tool in
  clippy) need cargo jq ;;
  deny) need cargo-deny jq ;;
  ast-grep) need ast-grep jq ;;
  *) usage ;;
esac
pack=$(dirname "$0")

# Lines of a pack list without comments or blanks.
entries() {
  sed -e 's/#.*//' -e 's/[[:space:]]*$//' -e '/^$/d' "$1"
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
report=$tmp/report
log=$tmp/log
list=
links=$tmp/links
echo '{}' >"$links"
status=0

case $tool in
  clippy)
    quality=skills/ns-contract/references/quality-bar.md
    sed -n 's/^\([a-z_:][a-z_:]*\)[[:space:]]*#[[:space:]]*\(.*[^[:space:]]\)[[:space:]]*$/\1\t\2/p' \
      "$pack/clippy-lints.txt" | jq -Rn '[inputs | split("\t") | {(.[0]): .[1]}] | add' >"$links"
    entries "$pack/clippy-lints.txt" >"$tmp/lints"
    set --
    while read -r lint; do
      set -- "$@" -W "$lint"
    done <"$tmp/lints"
    cargo clippy -q --manifest-path "$dir/Cargo.toml" --workspace --all-targets \
      --message-format=json -- -D warnings "$@" >"$report" 2>"$log" || status=$?
    filter='select(.reason == "compiler-message") | .message | select(.code or ($failed and .level == "error"))
      | (.code.code // "compile-error") as $rule
      | (.spans[] | select(.is_primary)) as $s
      | {tool: "clippy", rule: $rule, file: ($dir + "/" + $s.file_name),
         line: $s.line_start,
         message: (.message + " (quality bar" + ($links[0][$rule] | if . then ", " + . else "" end) + ": '$quality')")}'
    ;;
  deny)
    list=$pack/allowed-crates.txt
    if [ -z "$(entries "$list")" ]; then
      echo "check.sh: $list lists no crates, and cargo-deny would allow every crate" >&2
      exit 2
    fi
    {
      cat "$pack/deny.toml"
      printf '\n[bans]\nmultiple-versions = "deny"\nwildcards = "allow"\nallow = [\n'
      entries "$list" | sed 's/.*/  { crate = "&" },/'
      echo "]"
    } >"$tmp/deny.toml"
    cargo deny --manifest-path "$dir/Cargo.toml" --format json --config "$tmp/deny.toml" \
      check bans licenses >"$log" 2>"$report" || status=$?
    filter='select(.type == "diagnostic") | .fields
      | {tool: "cargo-deny", rule: .code, file: ($dir + "/Cargo.toml"),
         message: (.message + if .code == "not-allowed"
           then "; add it to \($list) only if no listed crate does its job (one road per job: ns-principle-pave-the-road)"
           else " (one road per job: ns-principle-pave-the-road)" end),
         snippet: (.graphs[0].Krate.name // .message)}'
    ;;
  ast-grep)
    {
      cat "$pack/ast-grep.yml"
      entries "$pack/roads.txt" | while read -r name home; do
        cat <<EOF
---
id: duplicate-helper
language: rust
severity: error
message: "\`$name\` already exists in $home; reuse it instead of a second one (one road per job: ns-principle-pave-the-road)"
rule:
  any: [{kind: function_item}, {kind: struct_item}, {kind: enum_item}, {kind: trait_item}, {kind: type_item}]
  has: {field: name, regex: '^$name\$'}
ignores: ["$home"]
EOF
      done
    } >"$tmp/rules.yml"
    ast-grep scan --inline-rules "$(cat "$tmp/rules.yml")" --json=stream "$dir" \
      >"$report" 2>"$log" || status=$?
    filter='{tool: "ast-grep", rule: .ruleId, file: .file, line: (.range.start.line + 1),
      message: .message}'
    ;;
esac

found=$(jq -cR --arg dir "$dir" --arg list "$list" --argjson failed "$([ "$status" -ne 0 ] && echo true || echo false)" --slurpfile links "$links" "fromjson? | $filter" "$report" | sort -u)
if [ -n "$found" ]; then
  printf '%s\n' "$found"
elif [ "$status" -ne 0 ]; then
  cat "$report" "$log" >&2
fi
exit "$status"
