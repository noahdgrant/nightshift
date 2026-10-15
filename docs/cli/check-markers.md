# `ns check-markers`

## `ns check-markers [path] [--human]`

Checks the human-review markers in the files `git ls-files` lists under `path` (default `.`). A region runs from a line containing `ns:human-review start`, optionally followed by `: <reason>`, through a line containing `ns:human-review end`, in any comment syntax. Every start needs an end, and none may nest. The merge step uses these regions to send a PR that changes one to a human ([docs/factory/merge.md](../factory/merge.md), Marked regions).

Prints `{"ok","files","errors":[{"file","line","message"}]}`. Exits 0 when the markers balance, 1 on an unbalanced or nested marker, and 2 when `path` is not a directory in a git repository. `--human` prints one readable line per error instead of JSON.
