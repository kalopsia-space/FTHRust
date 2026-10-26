#!/usr/bin/env bash
# Finds, per quality level, the lowest AV1 (VA-API) bitrate whose VMAF matches
# H.264 (VA-API) at that level's 1080p preset. Encoder settings mirror
# RawEncoder::open: bit_rate only (driver rate control), GOP = 2 x fps, no
# B-frames. Usage: run.sh OUTDIR SOURCE.mp4...
# Each source contributes a 10 s excerpt starting at 5 s.
set -euo pipefail
out=$1; shift
mkdir -p "$out"
dev=${VAAPI_DEVICE:-/dev/dri/renderD128}
levels=(low:10000 medium:25000 high:50000)
percents=(50 55 60 65 70 75 80 85 90)

encode() { # src codec kbps fps dst
  ffmpeg -nostdin -hide_banner -loglevel error -y -vaapi_device "$dev" \
    -ss 5 -t 10 -i "$1" -an -vf 'scale=1920:1080,format=nv12,hwupload' \
    -c:v "$2" -b:v "${3}k" -g $(( $4 * 2 )) -bf 0 "$5"
}

vmaf() { # dist src json -> "mean p5"
  ffmpeg -nostdin -hide_banner -loglevel error -i "$1" -ss 5 -t 10 -i "$2" -lavfi \
    "[0:v]crop=1920:1080:0:0,setpts=PTS-STARTPTS[d];[1:v]scale=1920:1080,setpts=PTS-STARTPTS[r];[d][r]libvmaf=log_fmt=json:log_path=$3:n_threads=$(nproc):n_subsample=2" \
    -f null - || { echo "vmaf failed on $1" >&2; exit 1; }
  python3 -I - "$3" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
s = sorted(f["metrics"]["vmaf"] for f in d["frames"])
print(f'{d["pooled_metrics"]["vmaf"]["mean"]:.3f} {s[max(0, len(s) * 5 // 100 - 1)]:.3f}')
PY
}

printf 'source level codec kbps vmaf_mean vmaf_p5\n' | tee "$out/results.txt"
for src in "$@"; do
  name=$(basename "$src" .mp4)
  fps=$(ffprobe -v error -select_streams v:0 -show_entries stream=avg_frame_rate -of csv=p=0 "$src")
  fps=$(python3 -I -c "from fractions import Fraction; print(round(Fraction('$fps')))")
  for lv in "${levels[@]}"; do
    level=${lv%%:*}; kbps=${lv##*:}
    f="$out/$name-h264-$kbps.mp4"
    encode "$src" h264_vaapi "$kbps" "$fps" "$f"
    score=$(vmaf "$f" "$src" "$f.json")
    echo "$name $level h264 $kbps $score" | tee -a "$out/results.txt"
    for p in "${percents[@]}"; do
      k=$(( kbps * p / 100 ))
      f="$out/$name-av1-$k.mp4"
      encode "$src" av1_vaapi "$k" "$fps" "$f"
      score=$(vmaf "$f" "$src" "$f.json")
      echo "$name $level av1 $k $score" | tee -a "$out/results.txt"
    done
    rm -f "$out/$name"-*.mp4
  done
done
