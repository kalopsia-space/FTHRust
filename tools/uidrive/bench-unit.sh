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

# docs-shots.sh used to remove ${DOCS_SHOTS_DIR:-/tmp/fthrust-docs} and /tmp/fthrust-docs-bench unasked; it now
# applies bench.sh's rules to both before it does anything else, and removes what it prepared when it ends.
docs_shots_refuses_a_directory_it_did_not_make() {
  mkdir -p "$WORK/docs-precious"
  echo keep > "$WORK/docs-precious/file"
  local out
  if out=$(DOCS_SHOTS_DIR="$WORK/docs-precious" "$HERE/docs-shots.sh" 2>&1); then echo "docs-shots.sh succeeded: $out"; exit 1; fi
  need "a refusal in the output (got: $out)" grep -q "refusing to reuse" <<<"$out"
  need "the directory's content to survive" test "$(cat "$WORK/docs-precious/file")" = keep
}

docs_shots_refuses_a_bench_directory_it_did_not_make_and_removes_the_first() {
  mkdir -p "$WORK/docs-precious2"
  echo keep > "$WORK/docs-precious2/file"
  local out
  if out=$(DOCS_SHOTS_DIR="$WORK/docs-first" DOCS_SHOTS_BENCH="$WORK/docs-precious2" "$HERE/docs-shots.sh" 2>&1); then
    echo "docs-shots.sh succeeded: $out"; exit 1
  fi
  need "a refusal in the output (got: $out)" grep -q "refusing to reuse" <<<"$out"
  need "the bench directory's content to survive" test "$(cat "$WORK/docs-precious2/file")" = keep
  need "the first directory, made for this run, to be removed" test ! -e "$WORK/docs-first"
}

docs_shots_refuses_symlinks_and_other_users_directories() {
  mkdir -p "$WORK/docs-real"
  echo keep > "$WORK/docs-real/file"
  ln -s "$WORK/docs-real" "$WORK/docs-link"
  local out
  if out=$(DOCS_SHOTS_DIR="$WORK/docs-link" "$HERE/docs-shots.sh" 2>&1); then echo "a symlinked DOCS_SHOTS_DIR was accepted: $out"; exit 1; fi
  need "the symlink to be named (got: $out)" grep -q "must not be a symlink" <<<"$out"
  if out=$(DOCS_SHOTS_DIR="$WORK/docs-ok" DOCS_SHOTS_BENCH="$WORK/docs-link" "$HERE/docs-shots.sh" 2>&1); then echo "a symlinked DOCS_SHOTS_BENCH was accepted: $out"; exit 1; fi
  need "the symlink to be named (got: $out)" grep -q "must not be a symlink" <<<"$out"
  need "the target's content to survive" test "$(cat "$WORK/docs-real/file")" = keep
  # /usr exists and is somebody else's. Only tried as an ordinary user: if the ownership rule were ever lost, root
  # would be the one to empty it.
  [ "$(id -u)" != 0 ] || return 0
  if out=$(DOCS_SHOTS_DIR=/usr "$HERE/docs-shots.sh" 2>&1); then echo "/usr was accepted: $out"; exit 1; fi
  need "the ownership to be named (got: $out)" grep -q "not owned by you" <<<"$out"
  need "/usr to be left alone" test -d /usr/bin
}

# The screenshots show both paths, so they stay the same from run to run: no mktemp names.
docs_shots_keeps_the_fixed_directory_names() {
  need "the app directory default" grep -q 'DOCS_SHOTS_DIR:-/tmp/fthrust-docs}' "$HERE/docs-shots.sh"
  need "the bench directory default" grep -q 'DOCS_SHOTS_BENCH:-/tmp/fthrust-docs-bench}' "$HERE/docs-shots.sh"
  need "no random directory names" test -z "$(grep -n 'mktemp' "$HERE/docs-shots.sh")"
}

docs_shots_cleans_up_when_it_fails_midway() {
  local stub=$WORK/stub-cargo out rc
  mkdir -p "$stub" "$WORK/docs-seed"
  printf '#!/bin/sh\nexit 3\n' > "$stub/cargo"
  chmod +x "$stub/cargo"
  out=$(PATH="$stub:$PATH" DOCS_SHOTS_DIR="$WORK/docs-a" DOCS_SHOTS_BENCH="$WORK/docs-b" "$HERE/docs-shots.sh" --seed "$WORK/docs-seed" 2>&1)
  rc=$?
  need "the failing command's status (got $rc: $out)" test "$rc" = 3
  need "the app directory removed" test ! -e "$WORK/docs-a"
  need "the bench directory removed" test ! -e "$WORK/docs-b"
  need "the seed folder, which is not ours, left alone" test -d "$WORK/docs-seed"
}

# A copy of docs-shots.sh in a small fake repository: a cargo that succeeds, two fake binaries, and a bench.sh that has the
# real prepare_bench_dir and, run as a command, leaves a log and a shot behind and exits with FAKE_BENCH_EXIT. That is the
# part of the script that matters here: what is left when the bench itself fails, and what when it works.
docs_shots_fake_repo() { # DIR
  local fake=$1
  mkdir -p "$fake/tools/uidrive/scenarios" "$fake/target/debug" "$fake/docs" "$fake/bin" "$fake/seed"
  cp "$HERE/docs-shots.sh" "$fake/tools/uidrive/"
  : > "$fake/tools/uidrive/scenarios/docs_shots.sh"
  printf '#!/bin/sh\n' > "$fake/target/debug/fthrust"
  cp "$fake/target/debug/fthrust" "$fake/target/debug/fthrust-engine"
  printf '#!/bin/sh\nexit 0\n' > "$fake/bin/cargo"
  {
    echo 'MARKER=.uidrive-bench'
    echo 'die() { echo "bench: $*" >&2; exit 1; }'
    sed -n '/^prepare_bench_dir() {/,/^}/p' "$HERE/bench.sh"
    echo 'if [ "${BASH_SOURCE[0]}" = "$0" ]; then'
    echo '  mkdir -p "$BENCH_DIR/logs" "$BENCH_DIR/shots"; echo "app log" > "$BENCH_DIR/logs/app.log"; echo png > "$BENCH_DIR/shots/001-hotkeys.png"'
    echo '  exit "${FAKE_BENCH_EXIT:-0}"'
    echo 'fi'
  } > "$fake/tools/uidrive/bench.sh"
  chmod +x "$fake/tools/uidrive/bench.sh" "$fake/tools/uidrive/docs-shots.sh" "$fake/bin/cargo"
}

docs_shots_keeps_the_bench_directory_when_the_bench_fails() {
  local fake=$WORK/fake-docs-fail out rc
  docs_shots_fake_repo "$fake"
  out=$(PATH="$fake/bin:$PATH" FAKE_BENCH_EXIT=3 DOCS_SHOTS_DIR="$fake/work" DOCS_SHOTS_BENCH="$fake/bench" \
    "$fake/tools/uidrive/docs-shots.sh" --seed "$fake/seed" 2>&1)
  rc=$?
  need "the bench's own exit status (got $rc: $out)" test "$rc" = 3
  need "the bench's log kept" test "$(cat "$fake/bench/logs/app.log")" = "app log"
  need "the bench's shot kept" test -f "$fake/bench/shots/001-hotkeys.png"
  need "the directory named in the message (got: $out)" grep -q "$fake/bench keeps the bench's logs/ and shots/" <<<"$out"
  need "the app directory, which holds the seed copy and the app, removed" test ! -e "$fake/work"
  need "no screenshot copied over docs/images" test -z "$(ls "$fake/docs" 2>/dev/null)"
  # the next run clears what was kept (it carries the marker) and leaves nothing behind when it works
  out=$(PATH="$fake/bin:$PATH" FAKE_BENCH_EXIT=0 DOCS_SHOTS_DIR="$fake/work" DOCS_SHOTS_BENCH="$fake/bench" \
    "$fake/tools/uidrive/docs-shots.sh" --seed "$fake/seed" 2>&1)
  rc=$?
  need "the second run to work (got $rc: $out)" test "$rc" = 0
  need "the shot copied over under its published name" test -f "$fake/docs/images/hotkeys.png"
  need "both directories removed after a good run" test ! -e "$fake/bench" -a ! -e "$fake/work"
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
  docs_shots_refuses_a_bench_directory_it_did_not_make_and_removes_the_first \
  docs_shots_refuses_symlinks_and_other_users_directories \
  docs_shots_keeps_the_fixed_directory_names \
  docs_shots_cleans_up_when_it_fails_midway \
  docs_shots_keeps_the_bench_directory_when_the_bench_fails \
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
