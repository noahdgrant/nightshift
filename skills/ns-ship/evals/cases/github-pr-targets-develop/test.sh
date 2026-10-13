#!/bin/sh
# Tests for scripts/widget.sh.
set -e
out=$(sh scripts/widget.sh title)
[ "$out" = "Widgets" ] || { echo "FAIL: title printed '$out'"; exit 1; }
echo "1 passed"
