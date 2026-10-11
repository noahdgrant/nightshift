#!/usr/bin/env bash
# Install or check the pinned tools CI runs, from scripts/ci-tools.toml. This script is the
# only reader of that list.
#
#   scripts/ci-tools.sh install [<tool>...]   CI installs each tool (default: all of them)
#   scripts/ci-tools.sh check [<tool>...]     fails on a missing or wrong-version tool,
#                                             naming the command that installs it
#
# A workflow step can't run taiki-e/install-action, so install writes those tools to
# $GITHUB_OUTPUT as `install-action=<tool>@<version>,...` for the step that does.
set -euo pipefail

list="$(dirname "${BASH_SOURCE[0]}")/ci-tools.toml"

case ${1:-} in
  install | check) mode=$1; shift ;;
  *) echo "usage: scripts/ci-tools.sh install|check [<tool>...]" >&2; exit 2 ;;
esac

# One tab-separated line per tool: name, version, install method, package, check command.
tools=$(python3 -I - "$list" "$@" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as f:
    tools = tomllib.load(f)
names = sys.argv[2:] or list(tools)
unknown = [n for n in names if n not in tools]
if unknown:
    print("ci-tools: not in scripts/ci-tools.toml:", *unknown, file=sys.stderr)
    sys.exit(2)
for n in names:
    t = tools[n]
    if t["install"] not in ("pip", "apt", "install-action"):
        print(f"ci-tools: {n}: install must be pip, apt or install-action, not {t['install']}", file=sys.stderr)
        sys.exit(2)
    print("\t".join([n, t["version"], t["install"], t.get("package", n), t["check"]]))
PY
)
[ -n "$tools" ] || exit 0

# The command a contributor runs to install a tool locally.
local_install() {
  local name=$1 version=$2 method=$3 package=$4
  case $method in
    pip) echo "uv tool install $package==$version" ;;
    apt)
      if [ "$package" = "$name" ]; then
        echo "sudo apt-get install $package"
      else
        echo "sudo apt-get install $package && sudo ln -sf /usr/bin/$package /usr/local/bin/$name"
      fi
      ;;
    install-action) echo "cargo install --locked $name@$version" ;;
  esac
}

if [ "$mode" = check ]; then
  failed=0
  while IFS=$'\t' read -r name version method package cmd; do
    fix="install it with: $(local_install "$name" "$version" "$method" "$package")"
    if ! command -v "${cmd%% *}" >/dev/null; then
      echo "ci-tools: $name is not on PATH; $fix" >&2
      failed=1
      continue
    fi
    if ! out=$($cmd 2>&1); then
      echo "ci-tools: \`$cmd\` failed: ${out##*$'\n'}; $fix" >&2
      failed=1
      continue
    fi
    # The version, not a longer one it is a prefix or suffix of: 18 matches 18.1.3.
    re="(^|[^0-9.])${version//./\\.}([^0-9]|$)"
    if ! [[ $out =~ $re ]]; then
      echo "ci-tools: $name is not version $version (\`$cmd\` printed: ${out%%$'\n'*}); $fix" >&2
      failed=1
    fi
  done <<<"$tools"
  exit "$failed"
fi

pip=() apt=() links=() action=()
while IFS=$'\t' read -r name version method package _; do
  case $method in
    pip) pip+=("$package==$version") ;;
    apt)
      apt+=("$package")
      [ "$package" = "$name" ] || links+=("$package:$name")
      ;;
    install-action)
      [ -n "${GITHUB_OUTPUT:-}" ] || {
        echo "ci-tools: $name installs with taiki-e/install-action, which runs only in GitHub Actions; locally: $(local_install "$name" "$version" "$method" "$package")" >&2
        exit 1
      }
      action+=("$name@$version")
      ;;
  esac
done <<<"$tools"

[ "${#pip[@]}" -eq 0 ] || python3 -m pip install "${pip[@]}"
if [ "${#apt[@]}" -gt 0 ]; then
  sudo apt-get update
  sudo apt-get install -y "${apt[@]}"
  for link in "${links[@]}"; do
    sudo ln -sf "/usr/bin/${link%%:*}" "/usr/local/bin/${link#*:}"
  done
fi
[ "${#action[@]}" -eq 0 ] || (IFS=,; echo "install-action=${action[*]}" >>"$GITHUB_OUTPUT")
