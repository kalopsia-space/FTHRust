# Shared scenario helpers. Scenarios print PASS or FAIL <step> and exit 0/1.
set -u
STEP=""
step() { STEP="$1"; echo "--- $1"; }
fail() { echo "FAIL ${1:-$STEP}"; uidrive shot "fail-${STEP// /_}" >/dev/null 2>&1 || true; uidrive log -n 15 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-160; exit 1; }
run() { "$@" || fail; }
names() { uidrive state --json | python3 -c "import json,sys; print('\n'.join(sorted(set(m['name'] for m in json.load(sys.stdin)['markers']))))"; }
wait_file() { # wait_file GLOB SECONDS
  local i; for i in $(seq 1 $(( $2 * 10 ))); do compgen -G "$1" >/dev/null && return 0; sleep 0.1; done; return 1; }
