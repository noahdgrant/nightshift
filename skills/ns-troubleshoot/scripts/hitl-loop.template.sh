#!/usr/bin/env bash
# HITL loop: a human performs the steps, you read what they capture.
# Usage: bash hitl-loop.template.sh
# step "<instruction>"      shows the instruction, waits for Enter
# capture VAR "<question>"  reads the answer into VAR
# Captured values print as KEY=VALUE at the end. Capture observations only; sign-in is a step.

set -euo pipefail

step() {
  printf '\n>>> %s\n' "$1"
  read -r -p "    [Enter when done] " _
}

capture() {
  local var="$1" question="$2" answer
  printf '\n>>> %s\n' "$question"
  read -r -p "    > " answer
  printf -v "$var" '%s' "$answer"
}

step "Open <URL> and sign in."

capture ERRORED "<Action>. Did it fail? (y/n)"

capture ERROR_MSG "Paste the error message (or 'none'):"

printf '\n--- Captured ---\n'
printf 'ERRORED=%s\n' "$ERRORED"
printf 'ERROR_MSG=%s\n' "$ERROR_MSG"
