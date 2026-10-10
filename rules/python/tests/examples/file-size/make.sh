#!/bin/sh
# Writes this rule's examples into $1: fail.py has 1001 lines, pass.py 1000, and
# pass-blank-tail.py 1000 lines followed by blank ones.
lines() { python3 -c "print('\n'.join(f'x{i} = {i}' for i in range($1)))"; }
lines 1000 > "$1/pass.py"
lines 1001 > "$1/fail.py"
{ lines 1000; printf '\n\n\n'; } > "$1/pass-blank-tail.py"
