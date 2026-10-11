#!/bin/sh
# Writes pass-stdlib-all.py into $1: one `import X` for each public name (no leading _) in
# the running Python's sys.stdlib_module_names, so dropping a name from
# utils/stdlib-module.yml, or a Python that adds one, gives a finding. The file is only
# scanned, never run, so `import this` and `import antigravity` do nothing.
python3 -I -c '
import sys
for name in sorted(sys.stdlib_module_names):
    if not name.startswith("_"):
        print(f"import {name}")
' > "$1/pass-stdlib-all.py"
