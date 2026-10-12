#!/usr/bin/env bash
# Checks the fakes in bin/ against a scratch repo. Run from anywhere: evals/fakes/systemd/test.sh
set -euo pipefail
# An inherited GIT_DIR (scripts/check-git-dir-decoy.sh) would send the scratch repo's git elsewhere.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX
bin=$(cd "$(dirname "$0")/bin" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/realbin"
printf '#!/bin/sh\necho "real ns $*"\n' > "$tmp/realbin/ns"
chmod +x "$tmp/realbin/ns"
export PATH="$bin:$tmp/realbin:$PATH"
cd "$tmp" && git init -q repo && cd repo
fake=$(git rev-parse --path-format=absolute --git-common-dir)/fake
fail() { echo "FAIL: $*" >&2; exit 1; }

# No unit: inactive, not installed, start refused.
[ "$(systemctl --user is-active ns-watch.service || true)" = inactive ] || fail "is-active on no unit"
systemctl --user is-active ns-watch.service >/dev/null && fail "is-active exits 0 on no unit"
systemctl --user cat ns-watch.service 2>/dev/null && fail "cat succeeds on no unit"
systemctl --user start ns-watch.service 2>/dev/null && fail "start succeeds on no unit"

# systemd-run marks its unit active, runs nothing.
out=$(systemd-run --user --unit=ns-watch-supervised -p OOMPolicy=continue ns watch --until 06:30)
[ "$out" = "Running as unit: ns-watch-supervised.service" ] || fail "systemd-run said $out"
[ "$(systemctl --user is-active ns-watch-supervised)" = active ] || fail "transient unit not active"
grep -q '^systemd-run --user --unit=ns-watch-supervised -p OOMPolicy=continue ns watch --until 06:30$' "$fake/calls.log" || fail "systemd-run not recorded"
[ "$(systemctl --user show ns-watch-supervised -p ActiveState --value)" = active ] || fail "show --value"
systemctl --user show ns-watch-supervised -p ActiveState,Result | grep -qx 'Result=success' || fail "show two props"
[ "$(systemctl --user show ns-watch-supervised -p ActiveState | wc -l)" = 1 ] || fail "show filters props"
[ "$(systemctl --user show ns-watch-supervised --property=Result)" = Result=success ] || fail "show --property="
[ "$(systemctl --user show ns-watch-supervised -pResult)" = Result=success ] || fail "show -pX"
systemctl --user show ns-watch-supervised -p ActiveEnterTimestamp --value | grep -Eq '^[A-Z][a-z]{2} [0-9]{4}-[0-9]{2}-[0-9]{2} [0-9:]{8} UTC$' || fail "start time"
systemctl --user list-units | grep -qx ns-watch-supervised.service || fail "list-units"

[ "$(systemd-run --user -u night2 ns watch)" = "Running as unit: night2.service" ] || fail "systemd-run -u"
[ "$(systemd-run --user ns watch)" = "Running as unit: run-u1.service" ] || fail "systemd-run default unit"
systemctl --user daemon-reload || fail "other verbs exit 0"

# An installed unit starts, shows, stops.
touch "$fake/units/ns-watch.service"
systemctl --user cat ns-watch.service >/dev/null || fail "cat on installed unit"
systemctl --user start ns-watch.service || fail "start installed unit"
systemctl --user status ns-watch.service >/dev/null || fail "status on active unit"
systemctl --user stop ns-watch.service
systemctl --user is-active ns-watch.service >/dev/null && fail "stop left the unit active"
systemctl --user status ns-watch.service >/dev/null && fail "status exits 0 on an inactive unit"
grep -q '^systemctl --user stop ns-watch.service$' "$fake/calls.log" || fail "stop not recorded"

# A failed unit keeps its Result until reset-failed.
systemctl --user stop ns-watch-supervised; echo 1 > "$fake/failed/ns-watch-supervised.service"
[ "$(systemctl --user is-active ns-watch-supervised || true)" = failed ] || fail "is-active on failed unit"
systemctl --user show ns-watch-supervised -p Result,ExecMainStatus | grep -qx 'Result=exit-code' || fail "failed Result"
systemctl --user reset-failed ns-watch-supervised
[ -e "$fake/failed/ns-watch-supervised.service" ] && fail "reset-failed left the unit failed"

# gh and journalctl answer empty.
[ "$(gh pr list --json number)" = "[]" ] || fail "gh"
journalctl --user -u ns-watch | grep -q 'No entries' || fail "journalctl"

# glab has no merge request until mr create opens one, which then reads merge-ready.
glab mr view -F json >/dev/null 2>&1 && fail "glab mr view before create"
url=$(glab mr create --source-branch ns/1-x --target-branch main --title t --description-file /dev/null --yes)
[ "$url" = https://gitlab.example.com/acme/widgets/-/merge_requests/1 ] || fail "glab mr create said $url"
grep -q '^glab mr create --source-branch ns/1-x ' "$fake/calls.log" || fail "glab mr create not recorded"
glab mr view -F json | grep -q '"detailed_merge_status": "mergeable"' || fail "glab mr view not mergeable"
glab mr view -F json | grep -q '"head_pipeline": {"id": 1, "status": "success"' || fail "glab mr view head_pipeline status"
git -c user.name=t -c user.email=t@localhost commit -q --allow-empty -m one
glab mr view -F json | grep -q "\"sha\": \"$(git rev-parse HEAD)\", \"has_conflicts\"" || fail "glab mr view sha"
glab mr view 1 | grep -qx 'state:	opened' || fail "glab mr view text"
glab ci status | grep -q success || fail "glab ci status"
[ "$(glab api projects/:id/merge_requests/1/discussions)" = "[]" ] || fail "glab api"
glab mr update 1 --ready || fail "glab mr update"
glab mr merge 1 2>/dev/null && fail "glab mr merge not refused"
glab mr approve 1 2>/dev/null && fail "glab mr approve not refused"

# ns refuses model runs and passes the rest on.
ns watch --until 06:30 2>/dev/null && fail "ns watch not refused"
[ "$(ns log --json)" = "real ns log --json" ] || fail "ns passthrough"
grep -q '^ns watch --until 06:30$' "$fake/calls.log" || fail "ns watch not recorded"
( PATH="$bin:/usr/bin:/bin"; ns log 2>/dev/null; [ $? = 127 ] ) || fail "ns with no real ns"

# Outside a git repo the state goes under $HOME/fake.
cd "$tmp" && HOME="$tmp/h" gh issue list >/dev/null
grep -q '^gh issue list$' "$tmp/h/fake/calls.log" || fail "state outside a repo"
echo "fakes: ok"
