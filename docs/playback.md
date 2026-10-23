# Playback

How a clip plays in the editor (plan 6a). This is the pipeline under the
clip viewer; the viewer itself, the timeline and export are plans 6b and
6c. Nothing here touches the originals: playback reads them, and the only
file it writes is a disposable proxy under `~/.fthr/playback`.

## Pipeline

```
clip.mp4 ──┬── VideoDecoder (fthrust-media) ──► video worker ──► frames ──► VideoTexture (wgpu) ──► egui
           │
           └── Mixer (fthrust-media) ─────────► audio worker ──► PcmQueue ──► PipeWireOutput
                                                                                 │
                                                        position_ms  ◄── decoded − buffered
```

- **Video** (`fthrust_media::playback::video`): the best video stream,
  decoded to tightly packed RGBA with presentation times in ms. `seek(ms)`
  seeks the container to the keyframe at or before the target and decodes
  forward, so the first frame returned is the first at or after the
  target (the last frame when the target is past the end). `frame_at`
  does one seek-and-decode for paused scrubbing and poster frames.
- **Audio** (`fthrust_media::playback::mixer`): one decoder per manifest
  track, each resampled to 48 kHz stereo f32, mixed with per-track gains
  and a master gain, then through an `atempo` chain for rates other than
  1.0 (stages within [0.5, 2.0], so 0.25 and 4.0 use two). Tracks that
  cannot be opened are reported and mixed as silence; the mixer fails only
  when no track decodes. `position_ms()` is the source time consumed.
- **Service** (`fthrust_ui::services::playback`): the audio worker pulls
  1024-frame blocks into a queue of at most 7200 frames (150 ms); the
  video worker decodes up to four frames ahead. `take_frame()` returns the
  newest queued frame whose time has come (≤ position + 16 ms). The
  position is the audio clock (source frames decoded minus buffered
  output frames × rate) while audio is live and playing, the paused
  position while paused, and a wall clock scaled by the rate otherwise. `tick()` is called from the UI loop: it collects worker
  events, applies the sync rule and reports `Eof` once.
- **Texture** (`fthrust_ui::video_texture`): one `Rgba8UnormSrgb` wgpu
  texture, rewritten in place with `write_texture` while the size holds
  and recreated (with a new egui `TextureId`) when it changes.

## Sync rule

Audio is the clock, as in the reference (`sync_to_video_position`). Each
tick compares the audio position with the time of the frame on screen:

| Drift (audio − video) | Not holding | Holding |
| --- | --- | --- |
| > 120 ms | suspend the output (hold) | keep holding |
| 40 … 120 ms | nothing | keep holding |
| −120 … 40 ms | nothing | resume |
| < −120 ms | reseek audio to the video position | reseek |

No decision is made for 0.75 s after a seek or rate change (the recovery
guard). A hold is lifted on pause, seek, rate change and resync. When the
video has run out the audio is never held; when the audio runs out first
the wall clock carries the remaining video; the clip ends when both are
done. Pausing seeks both workers to the paused position, so no queued
audio plays on and the frame shown is the one at the playhead.

## Rates and pitch

Rates run 0.25–4.0. With pitch preservation the mixer's `atempo` chain
keeps the output at 48 kHz and the audio clock advances `rate` source
frames per output frame. Without it the worker resamples each block
linearly (pitch shifts with the rate), as the reference's fallback did.

## Seek proxy

Clips whose keyframes are more than 2.1 s apart in the first 8 s (the
engine's rings have two-second GOPs, imports often do not) get a preview
copy with a keyframe every 30 frames: software H.264 at 16 Mb/s, video
only (audio always decodes from the original), streamed straight into a
uniquely named `<hash>.<pid>-<nanos>.partial.mp4`, validated (size and
duration) and only then renamed to `~/.fthr/playback/<sha256(identity)>.mp4` where
the identity is the source's device, inode, size and mtime plus the
recipe version (`gop30-v2`). The proxy is built on a worker after the
clip opens; the video decoder switches to it when it is ready
(`PlaybackEvent::ProxyReady`); audio always decodes the original. No proxy
for HDR (PQ/HLG) clips, clips over 300 s, or when the result would be
over 2 GiB or more than 0.2 s off the source duration. Before a build the
cache keeps the newest 8 entries within 2 GiB, removes partials older than
an hour (a dead build's leftovers; younger ones are builds in progress) and
anything else it finds in the directory; a final file that no longer
probes is rebuilt.

## Failure modes

| Event | Meaning |
| --- | --- |
| `AudioFailed(msg)` | the output could not start, the PipeWire stream errored or disconnected, or the decoder failed; reported once, then playback continues on the wall clock, video only |
| `SourceFailed(id)` | one manifest track is missing or undecodable; the rest play |
| `ProxyReady` | the video decoder now reads the proxy |
| `Eof` | the end was reached; `playing()` is false and the position is the duration |

## Tests

- `cargo test -p fthrust-media --test playback_mixer` (gains, mute,
  rate, position, seek, missing tracks).
- `cargo test -p fthrust-media --test playback_video` (order, seek,
  `frame_at`).
- `cargo test -p fthrust-media --test playback_proxy` (probe, cache path,
  build, prune).
- `cargo test -p fthrust-ui --test playback_service` (the sync rule and
  the service on a null output with a manual clock).
- `cargo test -p fthrust-ui --test playback_service pipewire_live -- --ignored`
  plays one second of a silent clip through PipeWire.
- `cargo test -p fthrust-ui --test video_texture` uploads frames to a
  headless wgpu device and reads them back.
