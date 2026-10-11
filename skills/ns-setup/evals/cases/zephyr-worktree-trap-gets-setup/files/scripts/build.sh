#!/bin/sh
# Build the demo app for native_sim from the registered zlogger checkout.
#
#   scripts/build.sh [build-dir]
#
# scripts/bootstrap.sh registers the checkout, once per clone. Output goes to
# the given dir or a fresh temp dir, never into the repo.
set -eu
: "${ZEPHYR_BASE:?set ZEPHYR_BASE to the zephyr tree}"

module=$(git -C "$(dirname "$0")" config --get zlogger.module) || {
	echo "build.sh: no zlogger checkout registered; run scripts/bootstrap.sh" >&2
	exit 1
}
out=${1:-$(mktemp -d "${TMPDIR:-/tmp}/zlogger-build.XXXXXX")}
echo "build.sh: building $module/app, output in $out" >&2
exec west build -b native_sim "$module/app" -d "$out"
