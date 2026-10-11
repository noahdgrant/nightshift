#!/bin/sh
# Register this checkout as the one scripts/build.sh builds. Run once per clone.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
git -C "$root" config zlogger.module "$root"
echo "bootstrap.sh: zlogger checkout is $root"
