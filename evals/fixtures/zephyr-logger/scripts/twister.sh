#!/bin/sh
# Run the ztest suites on native_sim against an external $ZEPHYR_BASE.
#
#   scripts/twister.sh                      every suite under tests/
#   scripts/twister.sh tests/lib/ringbuf_log
#   scripts/twister.sh tests/lib/ringbuf_log/src/main.c
#
# A file argument selects the suite (the nearest testcase.yaml) holding it.
# Arguments outside any suite are ignored; if none select a suite, every
# suite runs. Output goes to a fresh temp dir, never into the repo.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
: "${ZEPHYR_BASE:?set ZEPHYR_BASE to the zephyr tree}"

suites=""
for arg in "$@"; do
	dir=$arg
	[ -d "$dir" ] || dir=$(dirname "$dir")
	while [ "$dir" != "." ] && [ "$dir" != "/" ] && [ ! -f "$dir/testcase.yaml" ]; do
		dir=$(dirname "$dir")
	done
	if [ ! -f "$dir/testcase.yaml" ]; then
		echo "twister.sh: $arg is not in a test suite, skipping" >&2
		continue
	fi
	case " $suites " in
	*" -T $dir "*) ;;
	*) suites="$suites -T $dir" ;;
	esac
done
[ -n "$suites" ] || suites="-T tests"

out=$(mktemp -d "${TMPDIR:-/tmp}/zlogger-twister.XXXXXX")
echo "twister.sh: output in $out" >&2

# shellcheck disable=SC2086
exec "$ZEPHYR_BASE/scripts/twister" $suites -p native_sim \
	-x=ZEPHYR_EXTRA_MODULES="$root" -O "$out" --inline-logs
