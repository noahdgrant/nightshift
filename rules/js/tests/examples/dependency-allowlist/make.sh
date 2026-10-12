#!/bin/sh
# Write pass-builtins.mjs into $1: an import of every built-in module of the running Node, so
# the hand-kept list in utils/node-builtin.yml can't miss one.
set -eu
node -e '
const names = require("module").builtinModules.filter((m) => !m.startsWith("_"));
console.log(names.map((m, i) => `import * as m${i} from "${m}";`).join("\n"));
console.log(`export const all = [${names.map((_, i) => `m${i}`).join(", ")}];`);
' >"$1/pass-builtins.mjs"
