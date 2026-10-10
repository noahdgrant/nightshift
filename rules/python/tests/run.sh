#!/usr/bin/env bash
# Check each rule of the Python pack against its examples in tests/examples/<rule>/:
# each fail*.py must give a finding for <rule>, and each pass*.py no finding at all.
# A rule directory with a make.sh also has it write examples into a temp directory,
# for examples too long to commit.
#
#   rules/python/tests/run.sh
set -euo pipefail

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX

pack=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

for tool in ruff ast-grep; do
  command -v "$tool" >/dev/null || {
    echo "run.sh: $tool is not on PATH; install it with: uv tool install ${tool/ast-grep/ast-grep-cli}" >&2
    exit 1
  }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# The commands of stack-row.md, the rules row that ns-setup writes into stack.md, run on
# target $2 with pack $1. They exit non-zero when they find something.
findings() {
  local cmd
  while IFS= read -r cmd; do
    cmd=${cmd//\{pack\}/\"\$1\"}
    bash -c "${cmd//\{target\}/\"\$2\"}" _ "$1" "$2" 2>&1 || true
  done < <(grep -o '`[^`]*`' "$pack/stack-row.md" | tr -d '`')
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
  srcs=("$dir")
  if [ -e "$dir/make.sh" ]; then
    srcs+=("$tmp/$rule")
    mkdir -p "$tmp/$rule"
    "$dir/make.sh" "$tmp/$rule"
  fi
  for kind in fail pass; do
    found=0
    for src in "${srcs[@]}"; do
      for example in "$src"/"$kind"*.py; do
        [ -e "$example" ] || continue
        found=1
        check_example "$pack" "$rule" "$example"
      done
    done
    [ "$found" -eq 1 ] || {
      echo "FAIL $rule: no $kind*.py example" >&2
      failed=1
    }
  done
  checked=$((checked + 1))
done

# The shipped allowlist is empty, so the accept branch runs on a copy whose allowlist is
# `inventory`.
cp -r "$pack" "$tmp/allowlist-pack"
sed -i "s/(?:)/(?:inventory)/" "$tmp/allowlist-pack/utils/allowed-package.yml"
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
  printf '%s\n' "$long" | tee "$dir/long.py" "$dir/é.py" "$dir/ sp ace.py" "$dir/back\\slash.py" >"$dir/.hidden/long.py"
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
  for want in long.py é.py ' sp ace.py' 'back\slash.py'; do
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

for root in ".dot/proj" 'p&q|r' 'p\back'; do
  for mode in plain git; do
    dir="$tmp/odd-$mode/$root"
    mkdir -p "$dir/.hidden"
    printf '%s\n' "$long" | tee "$dir/long.py" >"$dir/.hidden/long.py"
    if [ "$mode" = git ]; then
      git -C "$dir" init -q
      echo '.hidden/' >"$dir/.gitignore"
    fi
    out=$("$pack/file-size.sh" "$dir" 2>&1)
    [ "$out" = "$dir/long.py:1001:1: error[file-size]: file is past 1000 lines; split it" ] || {
      printf 'FAIL file-size: %s root %s gave:\n%s\n' "$mode" "$root" "$out" >&2
      failed=1
    }
  done
done

decoy="$tmp/decoy"
git init -q "$decoy"
git_as -C "$decoy" commit -q --allow-empty -m decoy
before=$(git -C "$decoy" rev-parse HEAD)
for mode in plain git; do
  out=$(GIT_DIR="$decoy/.git" GIT_WORK_TREE="$decoy" "$pack/file-size.sh" "$tmp/size-$mode" 2>&1)
  grep -qF "$tmp/size-$mode/long.py:1001:1: error[file-size]" <<<"$out" || {
    printf 'FAIL file-size: %s directory under an inherited GIT_DIR gave:\n%s\n' "$mode" "$out" >&2
    failed=1
  }
done
[ "$(git -C "$decoy" rev-parse HEAD)" = "$before" ] || {
  echo "FAIL file-size: an inherited GIT_DIR was committed to" >&2
  failed=1
}

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
# A relative file name starts its line in the loop, so its leading space must survive.
out=$(cd "$dir" && "$pack/file-size.sh" ' sp ace.py' 2>&1)
grep -qxF " sp ace.py:1001:1: error[file-size]: file is past 1000 lines; split it" <<<"$out" || {
  printf 'FAIL file-size: a file name with a leading space gave:\n%s\n' "$out" >&2
  failed=1
}

ns="$pack/../../cli/target/debug/ns"
if [ -x "$ns" ]; then
  repo="$tmp/ns-repo"
  mkdir -p "$repo/docs/agents"
  git -C "$repo" init -q
  { printf '| Task | Command | Takes |\n|---|---|---|\n'; sed -e "s|{pack}|$pack|g" -e 's|{target}|.|g' "$pack/stack-row.md"; } >"$repo/docs/agents/stack.md"
  printf 'import os  # noqa\n' >"$repo/bare.py"
  out=$(cd "$repo" && timeout 60 "$ns" rules check 2>/dev/null) && rc=0 || rc=$?
  [ "$rc" -eq 1 ] && grep -qF '"findings": 1,' <<<"$out" && grep -qF '"file": "bare.py"' <<<"$out" && grep -qF 'error[suppression-reason]' <<<"$out" || {
    printf 'FAIL ns rules check: exit %s on a bare noqa gave:\n%s\n' "$rc" "$out" >&2
    failed=1
  }
else
  echo "run.sh: skipped the ns rules check test; build $ns with: cargo build --manifest-path cli/Cargo.toml" >&2
fi

[ "$failed" -eq 0 ] || exit 1
echo "rules/python: $checked rules and the allowlist, all examples pass"
