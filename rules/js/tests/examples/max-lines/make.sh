#!/bin/sh
# Write fail.js (1001 lines), pass.js (1000 lines) and two files of 900 code lines plus 101 blank
# or comment lines (fail-blank.js, fail-comment.js) into $1: blank and comment lines count.
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
for kind in blank comment; do
  {
    i=0
    while [ "$i" -lt 900 ]; do
      echo "export const x$i = $i;"
      i=$((i + 1))
    done
    i=0
    while [ "$i" -lt 101 ]; do
      if [ "$kind" = blank ]; then echo; else echo "// line $i"; fi
      i=$((i + 1))
    done
  } >"$1/fail-$kind.js"
done
