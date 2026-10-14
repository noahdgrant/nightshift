#!/bin/sh
set -e
log="$(git rev-parse --git-common-dir)/ns-eval-ci-local.log"
if [ -z "$(git status --porcelain --untracked-files=all)" ]; then state=clean; else state=dirty; fi
echo "$(git rev-parse HEAD) $state" >> "$log"
CI_LOCAL=1 python3 -m pytest -q
