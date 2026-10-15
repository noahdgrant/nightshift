#!/bin/sh
# A ratchet for `ns ratchet`: print the flash or RAM an ELF image uses, in bytes, as the one line
# on stdout, from its berkeley size report. Flash is text + data (the initial values of data are
# stored in flash); RAM is data + bss. SIZE names the size tool, `size` by default; a cross
# build needs its toolchain's, such as arm-zephyr-eabi-size.
#
#   ratchet-memory.sh flash|ram <elf>
set -eu

[ $# -eq 2 ] && { [ "$1" = flash ] || [ "$1" = ram ]; } || {
  echo "usage: $0 flash|ram <elf>" >&2
  exit 2
}
what=$1
elf=$2
size=${SIZE:-size}
command -v "$size" >/dev/null 2>&1 || {
  echo "ratchet-memory.sh: $size is not on PATH; set SIZE to the toolchain's size tool" >&2
  exit 1
}
[ -f "$elf" ] || {
  echo "ratchet-memory.sh: $elf is missing; build it first" >&2
  exit 1
}
report=$("$size" -B "$elf") || {
  echo "ratchet-memory.sh: $size failed on $elf" >&2
  exit 1
}
printf '%s\n' "$report" | awk -v what="$what" '
  NR == 1 { header = $1 == "text" && $2 == "data" && $3 == "bss" }
  NR == 2 && $1 ~ /^[0-9]+$/ && $2 ~ /^[0-9]+$/ && $3 ~ /^[0-9]+$/ {
    value = what == "flash" ? $1 + $2 : $2 + $3
    found = 1
  }
  END {
    if (!header || !found || NR != 2) exit 1
    printf "%.0f\n", value
  }' || {
  echo "ratchet-memory.sh: $size printed no berkeley size report for $elf" >&2
  exit 1
}
