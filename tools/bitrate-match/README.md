# bitrate-match

Finds the AV1 bitrate that matches H.264 quality, for `AV1_PRESET_PERCENT` in
`crates/fthrust-core/src/capture_config.rs`.

```sh
tools/bitrate-match/run.sh OUTDIR clip1.mp4 clip2.mp4 …
```

For each source it takes 10 s from the 5 s mark at 1920×1080. It encodes that
with `h264_vaapi` at the three 1080p presets (10, 25 and 50 Mbps), and with
`av1_vaapi` at 50–90% of each in 5% steps. The settings mirror the engine's
(`bit_rate` only, GOP 2 × fps, no B-frames). Every encode is scored with
libvmaf against the excerpt. A level's match is the lowest AV1 bitrate whose
mean VMAF is ≥ H.264's and whose 5th-percentile VMAF is no more than 1 point
below H.264's. The table uses the worst case across sources.

## Measured 2026-10-26

RX 7900 XTX (radeonsi, Mesa 26.2.5), FFmpeg n9.0.2. The sources were three of
the user's 1080p120 game clips: calm (15.7 Mbps source), mid (29.9 Mbps) and
high motion (40.5 Mbps).

| Level | H.264 | AV1 match per clip (calm / mid / high motion) | Chosen |
|---|---|---|---|
| Low | 10 Mbps | 5.5 / 5.0 / 5.0 Mbps | 55% → 5.5 Mbps |
| Medium | 25 Mbps | 17.5 / 16.25 / 15.0 Mbps | 70% → 17.5 Mbps |
| High | 50 Mbps | 45.0 / 37.5 / 35.0 Mbps | 90% → 45 Mbps |

At High both codecs sit near the VMAF ceiling (~98), so the calm clip only
reaches H.264's mean at 90%. The sources were already compressed H.264, so
scores near the top are compressed-on-compressed.
