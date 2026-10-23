#!/usr/bin/env bash
# The UI drive bench: a headless cage compositor on a private socket, a
# scratch home and runtime directory, the FTHRust app inside it.
#   bench.sh up   [--release] [--engine synthetic|real] [--size WxH] [--app PATH] [--settings FILE] [--cage PATH] [--renderer auto|gles2|vulkan|pixman]
# The renderer is cage's (the compositor is still headless: nothing is shown
# anywhere). `auto` picks gles2 when a GPU render node exists, so the app's
# Vulkan driver gets dmabuf buffers, and pixman otherwise (CI, with lavapipe).
#   bench.sh down
#   bench.sh run SCENARIO [up options]
# `up` prints eval-able BENCH=, BENCH_SOCKET=, BENCH_APP= lines. Nothing here
# reads the caller's WAYLAND_DISPLAY, XDG_RUNTIME_DIR or HOME for the app.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)

die() { echo "bench: $*" >&2; exit 1; }

cmd=${1:-}; shift || true

parse_up() {
  PROFILE=debug; ENGINE=synthetic; SIZE=1920x1080; APP=""; SETTINGS=""; CAGE=cage; RENDERER=auto
  while [ $# -gt 0 ]; do
    case "$1" in
      --release) PROFILE=release ;;
      --engine) ENGINE=$2; shift ;;
      --size) SIZE=$2; shift ;;
      --app) APP=$2; shift ;;
      --settings) SETTINGS=$2; shift ;;
      --cage) CAGE=$2; shift ;;
      --renderer) RENDERER=$2; shift ;;
      *) die "unknown option $1" ;;
    esac
    shift
  done
  [ -n "$APP" ] || APP="$ROOT/target/$PROFILE/fthrust"
}

bench_up() {
  parse_up "$@"
  command -v "$CAGE" >/dev/null 2>&1 || die "cage not found at '$CAGE' (pacman -S cage)"
  command -v grim >/dev/null 2>&1 || die "grim not found (needed for screenshots)"
  [ -x "$APP" ] || die "app binary not found: $APP (cargo build -p fthrust-ui -p fthrust-engine)"
  [ -x "$(dirname "$APP")/fthrust-engine" ] || die "engine binary missing next to $APP"
  BENCH=${BENCH_DIR:-/tmp/uidrive-$$}
  rm -rf "$BENCH"
  mkdir -p "$BENCH/home/.fthr" "$BENCH/clips" "$BENCH/shots" "$BENCH/logs"
  mkdir -m 700 "$BENCH/rt"
  # Seed settings: sounds off, clips under the bench, then the caller's file on top.
  # `${ROOT}` in the caller's file is the repository root.
  python3 - "$BENCH" "$SETTINGS" "$ROOT" <<'PY'
import json, sys, pathlib
bench, extra, root = sys.argv[1], sys.argv[2], sys.argv[3]
s = {"notification_sounds_enabled": False, "clips_directory": f"{bench}/clips"}
if extra:
    s.update(json.loads(pathlib.Path(extra).read_text().replace("${ROOT}", root)))
pathlib.Path(bench, "home", ".fthr", "settings.json").write_text(json.dumps(s))
PY
  case "$ENGINE" in
    synthetic) ENGINE_ENV="FTHRUST_SYNTHETIC_BACKEND=1 FTHR_CAPTURE_BACKEND=synthetic FTHRUST_SYNTHETIC_SIZE=$SIZE" ;;
    real) ENGINE_ENV="FTHR_CAPTURE_BACKEND=screencopy" ;;
    *) die "--engine must be synthetic or real" ;;
  esac
  # A private session bus with NO service directories: nothing can be
  # activated on it (no portal, no notification daemon), so portal calls
  # fail at once and never reach the user's session.
  DBUS_ENV=""
  if command -v dbus-daemon >/dev/null 2>&1; then
    cat > "$BENCH/bus.conf" <<XML
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=$BENCH/rt/bus</listen>
  <!-- Only the invoking user may connect (the socket dir is 0700 as well).
       The eavesdrop allows are the standard session-bus ones: without the
       bare one, dbus-daemon denies clients their own replies. -->
  <policy context="default">
    <allow user="$(id -un)"/>
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
XML
    DBUS_PID=$(env -i HOME="$BENCH/home" XDG_RUNTIME_DIR="$BENCH/rt" PATH="$PATH" \
      dbus-daemon --config-file="$BENCH/bus.conf" --fork --print-pid 2>/dev/null || true)
    if [ -n "$DBUS_PID" ]; then
      echo "$DBUS_PID" > "$BENCH/dbus.pid"
      DBUS_ENV="DBUS_SESSION_BUS_ADDRESS=unix:path=$BENCH/rt/bus"
    fi
  fi
  if [ "$RENDERER" = auto ]; then
    if ls /dev/dri/renderD* >/dev/null 2>&1; then RENDERER=gles2; else RENDERER=pixman; fi
  fi
  W=${SIZE%x*}; H=${SIZE#*x}
  # shellcheck disable=SC2086
  env -i \
    HOME="$BENCH/home" XDG_RUNTIME_DIR="$BENCH/rt" PATH="$PATH" \
    FTHRUST_UI_DUMP="$BENCH/ui.json" RUST_LOG=info RUST_BACKTRACE=1 \
    WLR_BACKENDS=headless WLR_RENDERER="$RENDERER" WLR_LIBINPUT_NO_DEVICES=1 \
    WLR_HEADLESS_OUTPUTS=1 CAGE_HEADLESS_WIDTH="$W" CAGE_HEADLESS_HEIGHT="$H" \
    $ENGINE_ENV $DBUS_ENV ${BENCH_EXTRA_ENV:-} \
    "$CAGE" -- "$APP" > "$BENCH/logs/app.log" 2>&1 &
  echo $! > "$BENCH/cage.pid"
  ln -sf "$BENCH/home/.fthr/logs/engine.log" "$BENCH/logs/engine.log"
  for _ in $(seq 1 300); do
    [ -s "$BENCH/ui.json" ] && break
    if ! kill -0 "$(cat "$BENCH/cage.pid")" 2>/dev/null; then
      tail -20 "$BENCH/logs/app.log" >&2
      BENCH=$BENCH bench_down
      die "cage or the app exited before the first frame"
    fi
    sleep 0.1
  done
  if [ ! -s "$BENCH/ui.json" ]; then
    tail -20 "$BENCH/logs/app.log" >&2
    BENCH=$BENCH bench_down
    die "no UI dump within 30 s"
  fi
  echo "BENCH=$BENCH"
  echo "BENCH_SOCKET=wayland-0"
  echo "BENCH_APP=$APP"
}

bench_down() {
  local b=${BENCH:-}
  [ -n "$b" ] || die "BENCH is not set"
  if [ -f "$b/cage.pid" ]; then
    local pid; pid=$(cat "$b/cage.pid")
    pkill -TERM -P "$pid" 2>/dev/null || true
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 30); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
    kill -KILL "$pid" 2>/dev/null || true
    pkill -KILL -P "$pid" 2>/dev/null || true
  fi
  if [ -f "$b/dbus.pid" ]; then
    kill -TERM "$(cat "$b/dbus.pid")" 2>/dev/null || true
    rm -f "$b/dbus.pid"
  fi
  rm -f "$b/cage.pid"
}

bench_run() {
  local scenario=${1:-}; shift || true
  [ -n "$scenario" ] && [ -f "$scenario" ] || die "usage: bench.sh run SCENARIO [up options]"
  local drv="$HERE/target/debug/uidrive"
  [ -x "$drv" ] || (cd "$HERE" && cargo build -q)
  local out; out=$(bench_up "$@")
  eval "$out"
  export BENCH BENCH_SOCKET BENCH_APP
  trap 'bench_down' EXIT
  trap 'exit 130' INT TERM
  export PATH="$HERE/target/debug:$PATH"
  local code=0
  bash "$scenario" || code=$?
  trap - EXIT
  bench_down
  echo "scenario exit $code (shots in $BENCH/shots, logs in $BENCH/logs)"
  exit $code
}

case "$cmd" in
  up) bench_up "$@" ;;
  down) bench_down ;;
  run) bench_run "$@" ;;
  *) die "usage: bench.sh up|down|run" ;;
esac
