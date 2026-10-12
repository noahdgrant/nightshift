#!/usr/bin/env bash
# Run the C pack's tests, test_rules_c.py, from any directory.
set -euo pipefail

exec python3 -I -m unittest discover -s "$(dirname "${BASH_SOURCE[0]}")"
