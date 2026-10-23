#!/usr/bin/env bash
# Checks the bench's hygiene promises: nothing is activatable on its bus,
# a failed start leaves nothing behind, and `down` leaves nothing behind.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
fail() { echo "FAIL $1"; exit 1; }
leftovers() { # processes carrying this bench's bus address (only our own are readable)
  local d=$1 p; for p in /proc/[0-9]*; do
    { tr '\0' '\n' < "$p/environ" | grep -q "DBUS_SESSION_BUS_ADDRESS=unix:path=$d/rt/bus"; } 2>/dev/null && echo "${p#/proc/}"; done; return 0; }
D=/tmp/uidrive-selfcheck
echo "--- failed start leaves nothing"
BENCH_DIR=$D "$HERE/bench.sh" up --cage /bin/false >/dev/null 2>&1 && fail "up succeeded with a broken cage"
[ -z "$(leftovers $D)" ] || fail "processes left after a failed start: $(leftovers $D)"
pgrep -f "dbus-daemon.*$D/" >/dev/null && fail "dbus-daemon left after a failed start"
echo "--- bus activates nothing"
out=$(BENCH_DIR=$D "$HERE/bench.sh" up) || fail "up"
eval "$out"
names=$(busctl --address="unix:path=$BENCH/rt/bus" call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ListActivatableNames 2>&1)
case "$names" in
  'as 1 "org.freedesktop.DBus"') ;;
  *) BENCH=$BENCH "$HERE/bench.sh" down; fail "activatable names on the bench bus: $names" ;;
esac
echo "--- down leaves nothing"
BENCH=$BENCH "$HERE/bench.sh" down
sleep 1
[ -z "$(leftovers $D)" ] || fail "processes left after down: $(leftovers $D)"
pgrep -f "dbus-daemon.*$D/" >/dev/null && fail "dbus-daemon left after down"
pgrep -f "$D/" >/dev/null && fail "processes still reference $D: $(pgrep -af "$D/" | head -3)"
echo PASS
