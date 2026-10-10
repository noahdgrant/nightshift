#!/usr/bin/env bash
# Check each rule of the Python pack against its examples in tests/examples/<rule>/:
# each fail*.py must give a finding for <rule>, and each pass*.py no finding at all.
# A rule directory with a make.sh has it write its examples into a temp directory,
# for examples too long to commit.
#
#   rules/python/tests/run.sh
set -euo pipefail

pack=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

for tool in ruff ast-grep; do
  command -v "$tool" >/dev/null || {
    echo "run.sh: $tool is not on PATH; install it with: uv tool install ${tool/ast-grep/ast-grep-cli}" >&2
    exit 1
  }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# The pack's commands as the rules row in docs/agents/stack.md runs them, on one file.
# Both exit non-zero when they find something.
findings() {
  ruff check --config "$pack/ruff.toml" --no-cache --output-format=concise "$1" 2>&1 || true
  ast-grep scan --report-style short -c "$pack/sgconfig.yml" "$1" 2>&1 || true
}

failed=0
checked=0
for dir in "$pack"/tests/examples/*/; do
  rule=$(basename "$dir")
  src=$dir
  if [ -e "$dir/make.sh" ]; then
    src="$tmp/$rule"
    mkdir -p "$src"
    "$dir/make.sh" "$src"
  fi
  for example in "$src"/fail*.py "$src"/pass*.py; do
    [ -e "$example" ] || {
      echo "FAIL $rule: no $(basename "$example") example" >&2
      failed=1
      continue
    }
    out=$(findings "$example")
    case $(basename "$example") in
      fail*)
        grep -qF -e ": $rule " -e "[$rule]" <<<"$out" && continue
        printf 'FAIL %s: %s gave no %s finding:\n%s\n' "$rule" "$example" "$rule" "$out" >&2
        ;;
      pass*)
        out=$(grep -E '^[^ :]+:[0-9]+:' <<<"$out") || continue
        printf 'FAIL %s: %s gave findings:\n%s\n' "$rule" "$example" "$out" >&2
        ;;
    esac
    failed=1
  done
  checked=$((checked + 1))
done

if [ "$checked" -eq 0 ]; then
  echo "run.sh: no examples under $pack/tests/examples" >&2
  exit 1
fi
[ "$failed" -eq 0 ] || exit 1
echo "rules/python: $checked rules, all examples pass"
