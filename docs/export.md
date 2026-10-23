# Export and share

Plan 6c. The reference built FFmpeg command lines; FTHRust runs the same
filter graphs in-process through FFmpeg's libraries (`ffmpeg-next`), so
there is no `ffmpeg` binary to find or bundle. The reference digest is
`docs/superpowers/notes-export-reference.md`.

## Pipeline

```
editor edit ──► fthrust-core::export::graph::plan ──► GraphPlan
                (ExportSpec: kept ranges, crop,       (filter graph text, maps,
                 effects, stretch, speed, mix,         copy/encode, seek, -t)
                 watermark, preset filters)
GraphPlan ──► fthrust-media::export::run ──► staged file ──► validate ──► rename
              decode → avfilter → encode → mux    (.{stem}.partial.{hex}.ext)
```

- **Graph text** (`crates/fthrust-core/src/export/`): the reference's
  `_build_export_cmd` strings, label for label: several kept ranges trim
  and concat (`[v0][a0][v1][a1]concat=…`); one range seeks and either
  stream-copies or filters. Video chain order: crop (even size), curves,
  hue, colorbalance, unsharp, vignette, stretch scale + `setsar=1`, speed
  `setpts`. Audio: the manifest tracks mix through `amix` with the
  per-source gains, a `1/sqrt(n)` normaliser, a limiter and the master
  volume; speed is `atempo` (or `asetrate` when pitch follows speed).
- **Runner** (`crates/fthrust-media/src/export/`): inputs named `[0:v]` and
  `[0:a:N]` become `buffer`/`abuffer` sources (an input used twice is split
  first, as the CLI does), every output label gets a sink adapter (`fps` to
  the source rate and the encoder's pixel format; `aformat` 48 kHz stereo;
  `trim`/`atrim` for a single range's length). Video encodes through the
  capture encoder (NVENC, VA-API, software H.264 at 16 / 32 Mbit/s for Very
  fast / High quality, or a preset's bitrate); Lossless is FFV1 level 3 and
  FLAC in Matroska. A hardware encoder failure retries once in software
  ("Software fallback · software"). Progress is output time over the
  expected length, capped at 99 % until the file validates.
- **Commit**: the staged file must hold a video stream whose first frame
  decodes within 15 s, and must not exceed a size preset's target ("Output
  is X MB, above the N MB target"); then it is renamed into place. Cancel,
  a 120 s stall, or any failure removes the staged file.

## Queue

`crates/fthrust-ui/src/services/export.rs`. Editor exports are admitted
first-in first-out, one at a time, or 2–4 at once with "Allow simultaneous
exports". Records keep the newest 50 plus anything still running. The
status line walks Queued → Preparing → `Exporting · {encoder}` → Completed
/ Failed / Cancelled / Timed_Out. Quitting cancels every export and waits
at most 12 s.

The editor's sidebar shows `Export jobs · n pending|failed|completed`
(refreshed each second); clicking it opens the **Exports** window (also
Settings → Performance → View export jobs): rows with progress, the
failure text as the status tooltip, "Cancel selected" and "Open output
folder".

Output: `{clips}/Exported/{stem}_export_{12 hex}.mp4` (`.mkv` lossless).

## Share

The Share button opens the picker: quality, "Share clip", "10 MB ·
Discord", and custom presets with "Use". Lossless cannot be combined with
a size preset. Each share runs in its own window (not listed in Exports)
through the queue's slots: `{clips}/Shared/{stem}_share_{HH-MM-SS}_{8
hex}.mp4` (`_discord_` for Discord). A preset share probes the clip, shows
the plan summary ("1920×1080→1280×720 · 89.4 MB→≤9.3 MB · AAC 96 kbps
audio"), scales and caps the bitrate, and fails visibly when the result
is still too big. A simple share (one range, no effects, watermark,
stretch or speed) keeps every audio track; otherwise the editor's mix is
used. Closing a share window cancels it.

## Live preview

`crates/fthrust-ui/src/editor/preview.rs` and `preview.wgsl`. The video is
stretched horizontally like the export; outside the crop is veiled, the
crop is outlined in the accent colour and labelled `CROP · w × h`. Colour
effects run through a WGSL port of the reference's `color.frag` while any
effect is non-zero. `clip_editor_live_preview` off shows the original
video; `editor_color_preview_enabled` off keeps crop and stretch only. The preview is an
approximation of the FFmpeg filters by design, as in the reference.

## Differences from the reference

See `CHANGES.md`: no CLI; Linux encoders only; the share window offers
"Copy path" / "Open folder" instead of a file drag (winit cannot start one
on Wayland); speed applies per range in concat exports; leaving an editor
is refused only while one of its shares runs; egui's built-in Ctrl+Q quit
and keyboard zoom are off. The reference's `FFMPEG NOT FOUND` error cannot
occur. As in the reference, the editor reports completion only through
the jobs label.

## Tests

- `cargo test -p fthrust-core --test export_model` (graphs, names, plans)
- `cargo test -p fthrust-media --test export_run` (real exports: concat with
  mix and speed, stream copy, lossless, cancel, missing source, read-only
  folder, no audio, software fallback, media probe)
- `cargo test -p fthrust-ui --test export_queue --test export_ui --test preview`
- `cargo test -p fthrust-ui --test fidelity_all export_jobs share`
- `tools/uidrive/bench.sh run tools/uidrive/scenarios/export.sh`
