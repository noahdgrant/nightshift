#!/bin/sh
# A ratchet for `ns ratchet`: run the tests under pytest-cov and print the total coverage, a
# percentage, as the one line on stdout. pytest's own output goes to stderr. The arguments go
# to pytest; with none, `--cov` measures every file under the current directory.
#
#   ratchet-coverage.sh [pytest args...]
set -eu

command -v pytest >/dev/null 2>&1 || {
  echo "ratchet-coverage.sh: pytest is not on PATH" >&2
  exit 1
}
[ "$#" -gt 0 ] || set -- --cov
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

pytest -q --cov-report="json:$tmp/coverage.json" "$@" >&2 || {
  rc=$?
  echo "ratchet-coverage.sh: pytest exited $rc, so there is no coverage to report" >&2
  exit 1
}
python3 -I - "$tmp/coverage.json" <<'PY' || {
import json, sys
try:
    with open(sys.argv[1]) as f:
        total = json.load(f)["totals"]["percent_covered"]
except (OSError, ValueError, KeyError, TypeError):
    sys.exit(1)
print(total)
PY
  echo "ratchet-coverage.sh: no total coverage in the report pytest wrote; is pytest-cov installed, and is a --cov argument given?" >&2
  exit 1
}
