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
#
# A download tool's zip is kept in ${XDG_CACHE_HOME:-~/.cache}/nightshift/ (rules/java/install.sh
# reads PMD's from there), unpacked under ${XDG_DATA_HOME:-~/.local/share}/nightshift/, and its
# bin linked into ~/.local/bin, which install adds to $GITHUB_PATH.
set -euo pipefail

list="$(dirname "${BASH_SOURCE[0]}")/ci-tools.toml"

case ${1:-} in
  install | check) mode=$1; shift ;;
  *) echo "usage: scripts/ci-tools.sh install|check [<tool>...]" >&2; exit 2 ;;
esac

python3 -I -c 'import sys; sys.exit(sys.version_info < (3, 11))' 2>/dev/null || {
  echo "ci-tools: needs Python 3.11 or newer (for tomllib); python3 on PATH is older" >&2
  exit 1
}

# One tab-separated line per tool: name, version, install method, package (a download's url),
# check command, bin and sha256. bin and sha256 come last, as tabs between empty fields collapse.
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
    if t["install"] not in ("pip", "apt", "install-action", "download"):
        print(f"ci-tools: {n}: install must be pip, apt, install-action or download, not {t['install']}", file=sys.stderr)
        sys.exit(2)
    package = t.get("url", t.get("package", n)).replace("{version}", t["version"])
    bin = t.get("bin", "/usr/bin/" + package if t["install"] == "apt" else "").replace("{version}", t["version"])
    print("\t".join([n, t["version"], t["install"], package, t["check"], bin, t.get("sha256", "")]))
PY
)
[ -n "$tools" ] || exit 0

# The command a contributor runs to install a tool locally.
local_install() {
  local name=$1 version=$2 method=$3 package=$4 bin=$5
  case $method in
    pip) echo "uv tool install $package==$version" ;;
    apt)
      if [ "$bin" = "/usr/bin/$name" ]; then
        echo "sudo apt-get install $package"
      else
        echo "sudo apt-get install $package && sudo ln -sf $bin /usr/local/bin/$name"
      fi
      ;;
    install-action) echo "cargo install --locked $name@$version" ;;
    download) echo "scripts/ci-tools.sh install $name" ;;
  esac
}

# Fetch a download tool's zip unless the cache has it, check it against the pinned sha256, unpack
# it, and link its bin into ~/.local/bin.
download() {
  local name=$1 version=$2 url=$3 bin=$4 sha=$5
  local cache=${XDG_CACHE_HOME:-$HOME/.cache}/nightshift
  local archive=$cache/${url##*/}
  local dir=${XDG_DATA_HOME:-$HOME/.local/share}/nightshift/$name-$version
  mkdir -p "$cache" "${dir%/*}" "$HOME/.local/bin"
  if [ ! -f "$archive" ]; then
    if ! curl -fsSL --retry 3 -o "$archive.part" "$url"; then
      rm -f "$archive.part"
      echo "ci-tools: cannot download $name from $url" >&2
      exit 1
    fi
    mv "$archive.part" "$archive"
  fi
  if ! echo "$sha  $archive" | sha256sum -c --status -; then
    rm -f "$archive"
    echo "ci-tools: $archive does not match the sha256 pinned for $name; removed it, so a rerun downloads it again" >&2
    exit 1
  fi
  if [ ! -x "$dir/$bin" ]; then
    rm -rf "$dir" "$dir.part"
    unzip -q "$archive" -d "$dir.part"
    mv "$dir.part" "$dir"
  fi
  ln -sf "$dir/$bin" "$HOME/.local/bin/$name"
  [ -z "${GITHUB_PATH:-}" ] || echo "$HOME/.local/bin" >>"$GITHUB_PATH"
}

if [ "$mode" = check ]; then
  failed=0
  while IFS=$'\t' read -r name version method package cmd bin _; do
    fix="install it with: $(local_install "$name" "$version" "$method" "$package" "$bin")"
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

pip=() apt=() links=() action=() downloads=()
while IFS=$'\t' read -r name version method package _ bin sha; do
  case $method in
    pip) pip+=("$package==$version") ;;
    apt)
      apt+=("$package")
      # Linked even when bin is /usr/bin/<name>, so it comes before a runner's preinstalled copy
      # in /usr/local/bin (the runner ships its own npm).
      links+=("$bin:$name")
      ;;
    download) downloads+=("$name"$'\t'"$version"$'\t'"$package"$'\t'"$bin"$'\t'"$sha") ;;
    install-action)
      [ -n "${GITHUB_OUTPUT:-}" ] || {
        echo "ci-tools: $name installs with taiki-e/install-action, which runs only in GitHub Actions; locally: $(local_install "$name" "$version" "$method" "$package" "$bin")" >&2
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
    sudo ln -sf "${link%:*}" "/usr/local/bin/${link##*:}"
  done
fi
for d in "${downloads[@]}"; do
  IFS=$'\t' read -r name version url bin sha <<<"$d"
  download "$name" "$version" "$url" "$bin" "$sha"
done
[ "${#action[@]}" -eq 0 ] || (IFS=,; echo "install-action=${action[*]}" >>"$GITHUB_OUTPUT")
