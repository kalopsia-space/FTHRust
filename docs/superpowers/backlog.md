# Backlog

Work we chose to defer, with what we knew when we deferred it.

## Performance (after 2.0.0-alpha.2)

### Editor: skip conversions for frames that are never shown

The playback video worker converts every decoded frame to RGBA
(`crates/fthrust-ui/src/services/playback/mod.rs`, `video_loop` →
`VideoDecoder::next_frame`), but `take_frame` only shows the newest frame due
at each UI repaint and drops the rest. When the clip's frame rate is above the
repaint rate, the dropped frames were converted for nothing: two thirds of the
conversions for a 180 fps clip on a 60 Hz screen, about a fifth on 144 Hz,
none on 180 Hz (the developer's monitors, so it was not urgent).

Sketch: the service measures the repaint interval (time between `take_frame`
calls while playing) and shares a minimum frame gap with the worker; the
worker skips conversion (decode only) of frames closer than that to the last
converted one, reusing `VideoDecoder`'s seek-target skip path. Keep the gap
at `floor(interval / frame duration)` frames so every repaint still has a new
frame. Paused, seek and end-of-clip frames always convert.

Superseded by the GPU path below if that lands first.

### Editor: colour conversion on the GPU

Each 1080p frame is converted to RGBA on the CPU (swscale) and copied into a
new 8 MB `Vec` before upload. Uploading the YUV planes and converting in a
shader (BT.709, limited range, as `video_texture.rs` already handles) removes
both; VA-API decode could follow. Decoding is no longer the limit (399 fps
with 4 frame threads, 2026-10-26), so this is about CPU use and heat.

### Export: keep the pipeline on the GPU

Export decodes on the CPU (frame threads since 2026-10-26) and encodes on the
GPU. Hardware decode and `scale_vaapi` would keep frames on the GPU. Not
measured end to end yet.

### Not measured yet

- UI CPU when idle and while scrolling a large library.
- Startup time; thumbnail generation for many clips.
- Disk replay buffer RAM savings on a real desktop (the plan 9 run used a
  tmpfs home).

## Packaging (from the 2026-10-31 audit)

### A trimmed FFmpeg for the AppImage

The AppImage ships BtbN's all-libraries LGPL FFmpeg build. It is most of the
86 MB image (uncompressed: libavcodec 69 MB, libavfilter 30 MB, libavformat
24 MB, libswscale 13 MB), and its ~60 external libraries make up most of
`THIRD_PARTY_NOTICES.md`. One of them, FFTW (GPL-2.0-or-later, pulled in by
chromaprint), makes `libavformat` GPL-covered; that is compatible with the
app's GPL-3.0 and the notices say so, but nothing in the app uses chromaprint.

Deferred on 2026-10-31 rather than done with the audit fixes: it means
building FFmpeg and the encoder libraries from source in the packaging
container, and the hardware encoders it must keep (NVENC, VA-API, QSV, AMF)
can't be checked on the build machine. The current build works and is
licensed correctly, so the gain (size, a shorter notice list) didn't justify
the risk to the release.

Sketch: a custom variant of the pinned BtbN build scripts (the same
`scripts.d` steps, fewer of them) or a `--disable-everything` configure in
the packaging image, enabling only what the app uses:
- encoders: `h264/hevc/av1` over `_vaapi`, `_nvenc`, `_qsv`, `_amf`;
  `libopenh264`, `libkvazaar`, `libsvtav1`; `aac`;
- decoders: `h264`, `hevc`, `av1` (plus `libdav1d` if the native one is too
  slow), `aac`, `mjpeg`, `png`/`mjpeg` for posters and images;
- muxers/demuxers: `mp4`, `mov`, `matroska`/`webm` demux for imports;
- filters: `scale_vaapi`, `scale`, `format`, `aresample`, `amix`, `volume`,
  `atempo`, `setpts`, `overlay`, `concat`, plus what the exporter's graphs
  name (grep `fthrust-media/src/export`);
- devices: `v4l2`; hwaccels: `vaapi`, `cuda` (for NVENC frames), `qsv`.
Gate it with `packaging/verify-appdir.sh` and the notice generator (which
then lists only what is built), and keep the old build as a fallback until a
release has been checked on NVIDIA, AMD and Intel hardware.
