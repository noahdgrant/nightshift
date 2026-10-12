#!/bin/sh
# Write fail.js (1001 lines) and pass.js (1000 lines) into $1.
set -eu
for n in 1000 1001; do
  i=0
  while [ "$i" -lt "$n" ]; do
    echo "export const x$i = $i;"
    i=$((i + 1))
  done >"$1/$n.js"
done
mv "$1/1000.js" "$1/pass.js"
mv "$1/1001.js" "$1/fail.js"
