#!/usr/bin/env bash
# Hold the CPU with <workers> busy workers for <seconds>, to reproduce a flaky test under load
# (AGENTS.md, Tests). Run it in the background beside the test:
#
#   scripts/stress.sh 120 16 & cargo test --manifest-path cli/Cargo.toml --test x -- --test-threads=16
#
# It prints the workers' pids on stderr. When its time is up or it is interrupted, it stops
# exactly the children it started, never processes found by name. Each worker also stops itself
# a second after <seconds> (bash's SECONDS ticks in whole seconds), so even a SIGKILL of this
# script leaves none running past then. Started with & from a script, it inherits SIGINT
# ignored, as bash leaves it for a background job; stop it early with SIGTERM.
set -euo pipefail

usage() {
  echo "usage: scripts/stress.sh <seconds> <workers> (both positive integers)" >&2
  exit 2
}

[ "$#" -eq 2 ] || usage
for n in "$@"; do
  [[ "$n" =~ ^[1-9][0-9]*$ ]] || usage
done
seconds=$1
workers=$2

pids=()
# jobs -p, bash's own record of the children it started, also holds one forked just before a
# signal landed and not yet in any variable of this script.
stop() {
  kill $(jobs -p) 2>/dev/null || true
  wait
}
trap stop EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

end=$((SECONDS + seconds + 1))
for _ in $(seq "$workers"); do
  while ((SECONDS < end)); do :; done &
  pids+=("$!")
done
echo "stress: workers ${pids[*]}" >&2
sleep "$seconds" &
wait "$!"
