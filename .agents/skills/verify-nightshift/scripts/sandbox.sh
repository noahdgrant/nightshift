#!/usr/bin/env bash
# The only way to drive verify-nightshift's recipes: every command runs inside a sandbox.
#
#   sandbox.sh --new                    create a sandbox and print its directory, $V
#   "$V/sandbox" [--src] <command...>   run <command> inside the sandbox $V
#
# --new needs the ns built from this checkout (under $CARGO_TARGET_DIR when set, else cli/target)
# and the fakes in cli/tests/fixtures/factory/, and names whichever is missing before it creates
# anything. It makes $V with mktemp: a throwaway git repo $V/repo on main with a bare origin
# $V/remote.git, a throwaway HOME $V/home with the checkout's ns-* skills installed for claude,
# the ns config $V/config.toml, the fake claude, gh and glab, an ssh that always fails, the built
# ns, and $V/sandbox, a link to this script. Evidence goes in $OUT: the caller's when set, else
# .ns/verify/<run-id>/ in the checkout. Each --new makes its own $V, so two runs go side by side.
#
# "$V/sandbox" runs <command> in $V/repo, or in the checkout with --src, with $V/bin first on
# PATH, HOME, NS_CONFIG, FAKE_CTRL, FAKE_GH_DIR and TMPDIR in the sandbox, GIT_SSH_COMMAND=false,
# and the caller's forge tokens, model key, XDG_CONFIG_HOME, ssh agent and git location
# variables unset. V, NS_SRC, NS and OUT are set. <command> may be `cap <name> <command...>`,
# which saves the command, stdout, stderr and exit code as $OUT/<name>.{cmd,out,err,exit} and
# prints the exit code. Exits 2 with usage when there is no command or $V is not a sandbox, else
# with <command>'s exit code.
set -euo pipefail

usage() {
  echo "usage: sandbox.sh --new | \"\$V/sandbox\" [--src] <command...>" >&2
  exit 2
}

new() {
  local src ns target v f
  src=$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../../../.." && pwd)
  target=${CARGO_TARGET_DIR:-cli/target}
  case "$target" in /*) ;; *) target="$src/$target" ;; esac
  ns="$target/debug/ns"
  if [ ! -x "$ns" ]; then
    echo "sandbox.sh: no ns at $ns; run cargo build --manifest-path cli/Cargo.toml" >&2
    exit 1
  fi
  for f in "$src"/cli/tests/fixtures/factory/{claude,gh,glab}; do
    [ -f "$f" ] || { echo "sandbox.sh: no fake at $f" >&2; exit 1; }
  done
  v=$(mktemp -d -t ns-verify.XXXXXX)
  trap 'rm -rf -- "$v"' EXIT
  local out=${OUT:-$src/.ns/verify/$(date -u +%Y%m%dT%H%M%SZ)-${v##*.}}
  mkdir -p "$v/bin" "$v/home/.claude" "$v/ctrl" "$v/ghd" "$v/tmp" "$out"
  cp "$src"/cli/tests/fixtures/factory/{claude,gh,glab} "$v/bin/"
  printf '#!/bin/sh\necho "sandbox: ssh is cut off: ssh $*" >&2\nexit 255\n' > "$v/bin/ssh"
  chmod +x "$v/bin/ssh"
  ln -s "$ns" "$v/bin/ns"
  ln -s "$(readlink -f "${BASH_SOURCE[0]}")" "$v/sandbox"
  echo '{}' > "$v/home/.claude/.credentials.json"
  : > "$v/ghd/issues.jsonl"
  echo '[ "$1 $2" = "api user" ] && { echo octocat; exit 0; }' > "$v/ghd/hook.sh"
  printf '[roles.default]\nharness = "claude"\n[forge.github]\ntoken_command = "echo fake-token"\n' > "$v/config.toml"
  printf 'NS_SRC=%q\nNS=%q\nOUT=%q\n' "$src" "$ns" "$out" > "$v/env"
  "$v/sandbox" --src sh -c '
    git init -q --bare -b main "$V/remote.git"
    git init -q -b main "$V/repo"
    echo hi > "$V/repo/README"
    git -C "$V/repo" add README
    git -C "$V/repo" commit -qm init
    git -C "$V/repo" remote add origin "$V/remote.git"
    git -C "$V/repo" push -q origin main
    git -C "$V/repo" remote set-head origin main
    ns install --source "$NS_SRC/skills" --harness claude > "$V/install.json"
  '
  trap - EXIT
  echo "$v"
}

cap() {
  local n=$1
  shift
  printf '$ %s\n' "$*" > "$OUT/$n.cmd"
  local rc=0
  "$@" > "$OUT/$n.out" 2> "$OUT/$n.err" || rc=$?
  echo "$rc" > "$OUT/$n.exit"
  echo "$rc"
}

[ "$#" -gt 0 ] || usage
if [ "$1" = --new ]; then
  [ "$#" -eq 1 ] || usage
  new
  exit 0
fi

V=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
if [ ! -f "$V/env" ] || [ ! -L "$V/sandbox" ]; then
  echo "sandbox.sh: $V is not a sandbox; create one with sandbox.sh --new," \
    "then run \"\$V/sandbox\" <command...>" >&2
  exit 2
fi
. "$V/env"
dir="$V/repo"
if [ "$1" = --src ]; then
  dir=$NS_SRC
  shift
fi
[ "$#" -gt 0 ] || usage

unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN GH_HOST GH_CONFIG_DIR \
  GITLAB_TOKEN GITLAB_HOST ANTHROPIC_API_KEY OPENAI_API_KEY CODEX_API_KEY XDG_CONFIG_HOME \
  SSH_AUTH_SOCK GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX GIT_CONFIG_GLOBAL
export V NS_SRC NS OUT PATH="$V/bin:$PATH" HOME="$V/home" NS_CONFIG="$V/config.toml" \
  FAKE_CTRL="$V/ctrl" FAKE_GH_DIR="$V/ghd" TMPDIR="$V/tmp" TZ=UTC GIT_SSH_COMMAND=false \
  GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t \
  GIT_COMMITTER_EMAIL=t@t
cd "$dir"
if [ "$1" = cap ]; then
  shift
  [ "$#" -ge 2 ] || usage
  cap "$@"
else
  exec "$@"
fi
