# Shared scenario helpers. Scenarios print PASS or FAIL <step> and exit 0/1.
set -u
STEP=""
step() { STEP="$1"; echo "--- $1"; }
fail() { echo "FAIL ${1:-$STEP}"; uidrive shot "fail-${STEP// /_}" >/dev/null 2>&1 || true; uidrive log -n 15 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-160; exit 1; }
run() { "$@" || fail; }
names() { uidrive state --json | python3 -c "import json,sys; print('\n'.join(sorted(set(m['name'] for m in json.load(sys.stdin)['markers']))))"; }
wait_file() { # wait_file GLOB SECONDS
  local i; for i in $(seq 1 $(( $2 * 10 ))); do compgen -G "$1" >/dev/null && return 0; sleep 0.1; done; return 1; }
scroll_to() { # scroll_to MARKER [AT]: wheel at AT (default 640,400) until MARKER's top sits in 200..560
  local at=${2:-640,400} i y
  for i in $(seq 1 60); do
    y=$(uidrive find "$1" 2>/dev/null | awk '{print int($2)}')
    [ -n "$y" ] || return 1
    if [ "$y" -gt 560 ]; then uidrive scroll "$at" 45 >/dev/null || return 1
    elif [ "$y" -lt 200 ]; then uidrive scroll "$at" -45 >/dev/null || return 1
    else return 0; fi
    sleep 0.2
  done
  return 1
}
