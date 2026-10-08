# usage: sh tests/run.sh [test files...]; default: every tests/test_*.sh
[ "$CALC_MODE" = strict ] || { echo "fixture env missing"; exit 9; }
files="$*"
[ -n "$files" ] || files=$(ls tests/test_*.sh)
for f in $files; do sh "$f" || exit 1; done
