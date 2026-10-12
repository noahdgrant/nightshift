#!/bin/sh
# Writes this rule's examples into $1, each a class of int fields: fail.java has 1001 lines,
# pass.java 1000, pass-blank-tail.java 1000 lines followed by blank ones, and
# fail-blank-head.java 1001 lines of which the first 10 are blank.
set -eu
lines() { awk -v n="$1" 'BEGIN { print "class Big {"; for (i = 1; i <= n - 2; i++) print "    int x" i " = " i ";"; print "}" }'; }
lines 1000 >"$1/pass.java"
lines 1001 >"$1/fail.java"
{ lines 1000; printf '\n\n\n'; } >"$1/pass-blank-tail.java"
{ printf '\n\n\n\n\n\n\n\n\n\n'; lines 991; } >"$1/fail-blank-head.java"
