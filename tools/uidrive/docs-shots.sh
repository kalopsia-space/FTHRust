#!/usr/bin/env bash
# Regenerates the screenshots in docs/images on the uidrive bench.
#   tools/uidrive/docs-shots.sh [--seed DIR]
# Without --seed it renders a small library of abstract demo clips with
# ffmpeg; pass a folder laid out like a clips root to use real clips instead.
# Needs the bench's tools plus ffmpeg, pipewire and wireplumber.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
OUT="$ROOT/docs/images"
WORK=${DOCS_SHOTS_DIR:-/tmp/fthrust-docs}
SEED=""
[ "${1:-}" = "--seed" ] && SEED=$2

rm -rf "$WORK"; mkdir -p "$WORK/app"
if [ -z "$SEED" ]; then
  SEED="$WORK/seed"
  mkdir -p "$SEED/Desktop" "$SEED/Neon Drift" "$SEED/Screenshots"
  clip() { # clip LAVFI OUT TONE STAMP
    ffmpeg -y -loglevel error -f lavfi -i "$1" -f lavfi -i "sine=frequency=$3:sample_rate=48000" \
      -t 8 -c:v libx264 -preset veryfast -pix_fmt yuv420p -r 60 -c:a aac -shortest "$2"
    touch -d "$4" "$2"
  }
  clip "gradients=size=1920x1080:rate=60:type=spiral:c0=0xf97316:c1=0xdb2777:c2=0x1e1b4b:c3=0x20e3a0:nb_colors=4:speed=0.04" \
    "$SEED/Desktop/desktop_clip_from_24Oct2026_21-04-55.mp4" 550 "2026-10-24 21:04"
  clip "gradients=size=1920x1080:rate=60:type=radial:c0=0x20e3a0:c1=0x1d4ed8:c2=0x7c3aed:c3=0x0f1a17:nb_colors=4:speed=0.03" \
    "$SEED/Desktop/desktop_clip_from_23Oct2026_19-12-08.mp4" 330 "2026-10-23 19:12"
  clip "mandelbrot=size=1920x1080:rate=60:start_x=-0.743643887037151:start_y=0.13182590420533:start_scale=0.02:end_scale=0.005" \
    "$SEED/Neon Drift/Neon Drift_clip_from_24Oct2026_22-31-40.mp4" 660 "2026-10-24 22:31"
  clip "life=size=480x270:rate=60:mold=10:life_color=#20e3a0:death_color=#0b1512:mold_color=#155e48:ratio=0.2,trim=start=6,setpts=PTS-STARTPTS,scale=1920:1080:flags=neighbor" \
    "$SEED/Neon Drift/Neon Drift_clip_from_23Oct2026_20-47-13.mp4" 440 "2026-10-23 20:47"
  shot="$SEED/Screenshots/screenshot_from_20261024_210812_123456.png"
  ffmpeg -y -loglevel error -ss 3 -i "$SEED/Neon Drift/Neon Drift_clip_from_24Oct2026_22-31-40.mp4" -frames:v 1 "$shot"
  touch -d "2026-10-24 21:08" "$shot"
fi

# A neutral install path, so the Hotkeys popup doesn't show the checkout's.
cargo build -q -p fthrust-ui -p fthrust-engine --manifest-path "$ROOT/Cargo.toml"
cp "$ROOT/target/debug/fthrust" "$ROOT/target/debug/fthrust-engine" "$WORK/app/"

BENCH_DIR=/tmp/fthrust-docs-bench "$HERE/bench.sh" run "$HERE/scenarios/docs_shots.sh" \
  --engine real --pipewire --app "$WORK/app/fthrust" --seed "$SEED"

mkdir -p "$OUT"
for f in /tmp/fthrust-docs-bench/shots/*.png; do
  name=$(basename "$f"); name=${name#[0-9][0-9][0-9]-}
  cp "$f" "$OUT/$name"
done
echo "screenshots in $OUT"
