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
# They exit non-zero when they find something.
findings() {
  local p=$1
  ruff check --config "$p/ruff.toml" --no-cache --output-format=concise "$2" 2>&1 || true
  ast-grep scan --report-style short -c "$p/sgconfig.yml" "$2" 2>&1 || true
  "$p/file-size.sh" "$2" 2>&1 || true
}

# Check one example against pack $1 for rule $2: a fail*.py must give a finding for
# the rule, a pass*.py none at all. Sets failed=1 on a miss.
check_example() {
  local p=$1 rule=$2 example=$3 out
  out=$(findings "$p" "$example")
  case $(basename "$example") in
    fail*)
      grep -qF -e ": $rule " -e "[$rule]" <<<"$out" && return
      printf 'FAIL %s: %s gave no %s finding:\n%s\n' "$rule" "$example" "$rule" "$out" >&2
      ;;
    pass*)
      out=$(grep -E -e '^[^ :]+:[0-9]+:' -e '^Error' -e 'error:' <<<"$out") || return 0
      printf 'FAIL %s: %s gave findings:\n%s\n' "$rule" "$example" "$out" >&2
      ;;
  esac
  failed=1
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
    check_example "$pack" "$rule" "$example"
  done
  checked=$((checked + 1))
done

# The shipped allowlist is empty, so the accept branch runs on a copy whose allowlist is
# `inventory`, found by its marker.
marker='# the allowlist'
allow_rule=rules/dependency-allowlist.yml
[ "$(grep -cF -e "$marker" "$pack/$allow_rule")" -eq 1 ] || {
  echo "FAIL dependency-allowlist: $allow_rule needs exactly one '$marker' marker" >&2
  failed=1
}
cp -r "$pack" "$tmp/allowlist-pack"
sed -i "/$marker/s/(?:)/(?:inventory)/" "$tmp/allowlist-pack/$allow_rule"
grep -qF 'inventory' "$tmp/allowlist-pack/$allow_rule" || {
  echo "FAIL dependency-allowlist: the '$marker' line holds no empty allowlist to fill" >&2
  failed=1
}
for example in "$pack"/tests/allowlist/*.py; do
  check_example "$tmp/allowlist-pack" dependency-allowlist "$example"
done

# A rule's `ignores` lists the files it skips, which is how a helper's own module escapes
# duplicate-helper. The shipped list is empty, so a copy ignores `home.py` but not `other.py`.
cp -r "$pack" "$tmp/ignores-pack"
sed -i 's|^ignores: \[\]|ignores: ["**/home.py"]|' "$tmp/ignores-pack/rules/duplicate-helper.yml"
mkdir "$tmp/ignores"
printf 'def which(cmd):\n    pass\n' | tee "$tmp/ignores/home.py" >"$tmp/ignores/other.py"
out=$(findings "$tmp/ignores-pack" "$tmp/ignores")
grep -qF "other.py:1:1: error[duplicate-helper]" <<<"$out" || {
  printf 'FAIL duplicate-helper: ignores skipped other.py too:\n%s\n' "$out" >&2
  failed=1
}
if grep -qF "home.py" <<<"$out"; then
  printf 'FAIL duplicate-helper: ignores did not skip home.py:\n%s\n' "$out" >&2
  failed=1
fi

# file-size.sh on a directory, inside a git repo and outside one, reports a long file and
# skips a long file under a dot directory (outside git, by name; in git, by .gitignore).
long=$(python3 -c "print('\n'.join(f'x{i} = {i}' for i in range(1001)))")
git_as() { git -c user.name=t -c user.email=t@example.com "$@"; }
for mode in plain git; do
  dir="$tmp/size-$mode"
  mkdir -p "$dir/.hidden"
  printf '%s\n' "$long" | tee "$dir/long.py" "$dir/é.py" >"$dir/.hidden/long.py"
  if [ "$mode" = git ]; then
    git -C "$dir" init -q
    echo '.hidden/' >"$dir/.gitignore"
    printf '%s\n' "$long" >"$dir/tracked.py"
    git_as -C "$dir" add tracked.py
    git_as -C "$dir" commit -q -m tracked
    printf '%s\n' "$long" >"$dir/deleted.py"
    git_as -C "$dir" add deleted.py
    git_as -C "$dir" commit -q -m deleted
    rm "$dir/deleted.py"
  fi
  out=$("$pack/file-size.sh" "$dir" 2>&1)
  for want in long.py é.py; do
    grep -qF "$dir/$want:1001:1: error[file-size]" <<<"$out" || {
      printf 'FAIL file-size: %s directory gave no %s finding:\n%s\n' "$mode" "$want" "$out" >&2
      failed=1
    }
  done
  if grep -qF -e ".hidden" -e "deleted.py" -e "rror:" <<<"$out"; then
    printf 'FAIL file-size: %s directory gave:\n%s\n' "$mode" "$out" >&2
    failed=1
  fi
  if [ "$mode" = git ]; then
    grep -qF "$dir/tracked.py:1001:1: error[file-size]" <<<"$out" || {
      printf 'FAIL file-size: tracked file not reported:\n%s\n' "$out" >&2
      failed=1
    }
  fi
done

dir="$tmp/size-git"
out=$("$pack/file-size.sh" "$dir/" 2>&1)
grep -qF "$dir/long.py:1001:1: error[file-size]" <<<"$out" && ! grep -qF "//" <<<"$out" || {
  printf 'FAIL file-size: a trailing slash gave:\n%s\n' "$out" >&2
  failed=1
}
out=$(cd "$dir" && "$pack/file-size.sh" 2>&1)
grep -qF "./long.py:1001:1: error[file-size]" <<<"$out" && ! grep -qF "//" <<<"$out" || {
  printf 'FAIL file-size: no argument gave:\n%s\n' "$out" >&2
  failed=1
}

[ "$failed" -eq 0 ] || exit 1
echo "rules/python: $checked rules and the allowlist, all examples pass"
