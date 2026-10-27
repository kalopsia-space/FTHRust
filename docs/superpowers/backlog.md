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
