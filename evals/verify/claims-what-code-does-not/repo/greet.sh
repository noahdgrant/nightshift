#!/bin/sh
# Prints a greeting for the name given.
if [ $# -ne 1 ]; then
  echo "usage: greet.sh <name>" >&2
  exit 0
fi
echo "Hello, $1!"
