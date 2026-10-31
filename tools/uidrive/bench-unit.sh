#!/usr/bin/env bash
# Unit tests for the functions in bench.sh that need no compositor: the engine
# environment, the bench directory rules and the teardown. (bench-selfcheck.sh
# is the end-to-end check and needs cage; this one runs anywhere, CI included.)
#
#   tools/uidrive/bench-unit.sh
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=bench.sh
source "$HERE/bench.sh"
set +e # each case runs in its own subshell and checks its results itself

WORK=$(mktemp -d /tmp/uidrive-unit-XXXXXX)
trap 'rm -rf "$WORK"' EXIT
failures=0

check() { # NAME FUNCTION: runs the case in a subshell; its output only shows on failure
  local name=$1 out
  if out=$("$2" 2>&1); then echo "ok $name"; else echo "FAIL $name"; echo "$out" | sed 's/^/    /'; failures=$((failures + 1)); fi
}
need() { # DESCRIPTION CONDITION-COMMAND...: fails the case when the command fails
  local what=$1
  shift
  "$@" || { echo "expected: $what"; exit 1; }
}
gone() { # PID: true once the process has exited (a reaped zombie takes a moment)
  local _
  for _ in $(seq 1 30); do kill -0 "$1" 2>/dev/null || return 0; sleep 0.1; done
  return 1
}
refuses() { # COMMAND...: succeeds when the command fails
  local out
  if out=$("$@" 2>&1); then echo "expected a refusal, got success: $out"; return 1; fi
}

# What FTHR_CAPTURE_BACKEND accepts, read from the engine's own parser.
accepted_backends() {
  sed -n '/pub fn parse_override/,/^    }/p' "$ROOT/crates/fthrust-ipc/src/messages.rs" |
    sed -n 's/^ *"\([a-z0-9]*\)" =>.*/\1/p'
}

engine_modes_use_backend_names_the_engine_accepts() {
  local accepted mode env name
  accepted=$(accepted_backends)
  need "to read the accepted names from fthrust-ipc messages.rs" test -n "$accepted"
  for mode in synthetic real; do
    env=$(engine_env "$mode" 1280x720)
    name=$(grep -o 'FTHR_CAPTURE_BACKEND=[^ ]*' <<<"$env")
    name=${name#FTHR_CAPTURE_BACKEND=}
    need "$mode to set FTHR_CAPTURE_BACKEND (got: $env)" test -n "$name"
    need "$mode backend '$name' to be one of: $(tr '\n' ' ' <<<"$accepted")" grep -qx "$name" <<<"$accepted"
  done
}

engine_env_rejects_an_unknown_mode() {
  refuses engine_env bogus 1280x720
}

default_bench_dir_is_private_and_unpredictable() {
  unset BENCH_DIR
  prepare_bench_dir
  a=$BENCH
  prepare_bench_dir
  b=$BENCH
  trap 'rm -rf "$a" "$b"' EXIT # globals on purpose: the trap runs after this function returns
  need "two fresh directories" test "$a" != "$b"
  need "'$a' to look like /tmp/uidrive-XXXXXX" bash -c '[[ $1 =~ ^/tmp/uidrive-[A-Za-z0-9]{6}$ ]]' _ "$a"
  need "'$a' not to carry the shell pid" test "$a" != "/tmp/uidrive-$$"
  need "mode 700 on $a" test "$(stat -c %a "$a")" = 700
  need "the marker in $a" test -f "$a/$MARKER"
}

bench_dir_refuses_a_directory_it_did_not_make() {
  mkdir -p "$WORK/precious/sub"
  echo keep > "$WORK/precious/sub/file"
  export BENCH_DIR=$WORK/precious
  refuses prepare_bench_dir
  need "the directory's content to survive" test "$(cat "$WORK/precious/sub/file")" = keep
}

bench_dir_reuses_and_empties_its_own_directory() {
  export BENCH_DIR=$WORK/mine
  prepare_bench_dir
  need "the marker" test -f "$BENCH/$MARKER"
  need "mode 700" test "$(stat -c %a "$BENCH")" = 700
  mkdir -p "$BENCH/home/.fthr"
  touch "$BENCH/old" "$BENCH/home/.fthr/settings.json" "$BENCH/.hidden"
  prepare_bench_dir
  need "old files to be gone" test -z "$(find "$BENCH" -mindepth 1 ! -name "$MARKER")"
  need "the marker again" test -f "$BENCH/$MARKER"
}

bench_dir_accepts_an_empty_directory() {
  mkdir "$WORK/empty"
  export BENCH_DIR=$WORK/empty/
  prepare_bench_dir
  need "BENCH without a trailing slash" test "$BENCH" = "$WORK/empty"
  need "the marker" test -f "$BENCH/$MARKER"
}

bench_dir_strips_every_trailing_slash() {
  mkdir "$WORK/slashes"
  export BENCH_DIR="$WORK/slashes///"
  prepare_bench_dir
  need "BENCH without trailing slashes" test "$BENCH" = "$WORK/slashes"
  # "//" used to lose one slash and come out as "/"
  BENCH_DIR=// refuses prepare_bench_dir
  BENCH_DIR=/// refuses prepare_bench_dir
}

bench_dir_refuses_dot_dot() {
  mkdir -p "$WORK/dotdot/inner"
  touch "$WORK/dotdot/inner/$MARKER"
  BENCH_DIR="$WORK/dotdot/inner/.." refuses prepare_bench_dir
  BENCH_DIR="$WORK/dotdot/../dotdot/inner" refuses prepare_bench_dir
  need "the directory to be untouched" test -f "$WORK/dotdot/inner/$MARKER"
}

# docs-shots.sh used to remove ${DOCS_SHOTS_DIR:-/tmp/fthrust-docs} unasked; it now applies bench.sh's rules to
# that directory before it does anything else.
docs_shots_refuses_a_directory_it_did_not_make() {
  mkdir -p "$WORK/docs-precious"
  echo keep > "$WORK/docs-precious/file"
  local out
  if out=$(DOCS_SHOTS_DIR="$WORK/docs-precious" "$HERE/docs-shots.sh" 2>&1); then echo "docs-shots.sh succeeded: $out"; exit 1; fi
  need "a refusal in the output (got: $out)" grep -q "refusing to reuse" <<<"$out"
  need "the directory's content to survive" test "$(cat "$WORK/docs-precious/file")" = keep
}

bench_dir_creates_a_missing_directory() {
  export BENCH_DIR=$WORK/new/deeper
  prepare_bench_dir
  need "the directory" test -d "$WORK/new/deeper"
  need "the marker" test -f "$WORK/new/deeper/$MARKER"
}

bench_dir_refuses_root_relative_and_symlinks() {
  mkdir -p "$WORK/target"
  touch "$WORK/target/$MARKER" "$WORK/target/data"
  ln -s "$WORK/target" "$WORK/link"
  BENCH_DIR=/ refuses prepare_bench_dir
  BENCH_DIR=relative/dir refuses prepare_bench_dir
  BENCH_DIR=$WORK/link refuses prepare_bench_dir
  BENCH_DIR=$WORK/target/data refuses prepare_bench_dir
  need "the symlink target to be untouched" test -f "$WORK/target/data"
}

# Dead pids in every pid file: the old teardown ended at the first failed kill
# (set -e), leaving the bus daemon and the pid files behind.
bench_down_survives_dead_processes() {
  local dead d
  sleep 0 &
  dead=$!
  wait "$dead"
  mkdir -p "$WORK/stale"
  for d in cage wireplumber pipewire dbus; do echo "$dead" > "$WORK/stale/$d.pid"; done
  ( set -e; BENCH=$WORK/stale; bench_down ) || { echo "bench_down failed with dead pids"; exit 1; }
  need "no pid files left" test -z "$(ls "$WORK/stale"/*.pid 2>/dev/null)"
  # and unreadable or empty pid files
  : > "$WORK/stale/pipewire.pid"
  ( set -e; BENCH=$WORK/stale; bench_down ) || { echo "bench_down failed with an empty pid file"; exit 1; }
}

bench_down_stops_the_compositor_and_its_children() {
  local pid child
  mkdir -p "$WORK/live"
  bash -c 'sleep 300 & wait' &
  pid=$!
  echo "$pid" > "$WORK/live/cage.pid"
  sleep 0.3
  child=$(pgrep -P "$pid" | head -1)
  need "a child process to stop" test -n "$child"
  ( set -e; BENCH=$WORK/live; bench_down ) || { echo "bench_down failed"; exit 1; }
  need "the compositor stopped" gone "$pid"
  need "its child stopped" gone "$child"
  need "cage.pid removed" test ! -e "$WORK/live/cage.pid"
}

bench_down_needs_a_bench() {
  ( unset BENCH; bench_down ) 2>/dev/null && { echo "bench_down without BENCH succeeded"; exit 1; }
  return 0
}

for t in \
  engine_modes_use_backend_names_the_engine_accepts \
  engine_env_rejects_an_unknown_mode \
  default_bench_dir_is_private_and_unpredictable \
  bench_dir_refuses_a_directory_it_did_not_make \
  bench_dir_reuses_and_empties_its_own_directory \
  bench_dir_accepts_an_empty_directory \
  bench_dir_creates_a_missing_directory \
  docs_shots_refuses_a_directory_it_did_not_make \
  bench_dir_strips_every_trailing_slash \
  bench_dir_refuses_dot_dot \
  bench_dir_refuses_root_relative_and_symlinks \
  bench_down_survives_dead_processes \
  bench_down_stops_the_compositor_and_its_children \
  bench_down_needs_a_bench; do
  check "$t" "$t"
done
[ $failures -eq 0 ] && echo "all ok" || echo "$failures failed"
[ $failures -eq 0 ]
