# Capture overlays

Plan 6d. Reference digests: `docs/superpowers/notes-overlays-reference.md`
(webcam, image layers, Gary) and `docs/superpowers/notes-pickers-reference.md`
(file and folder pickers).

## After a save: image layers and the webcam

When `camera_enabled` is on or any image layer is enabled, a saved clip stays
`Finalizing` (its card shows the badge; opening it shows `CLIP STILL
FINALIZING`) while one worker (`services::finalize`) rewrites it:

1. Missing layer files and files FFmpeg cannot decode are dropped, each with
   a warning (`Image overlay source is missing: …`, `Image overlay could not
   be read: …`).
2. The camera window `[save − length, save]` is written from the ring as an
   MJPEG Matroska file with each frame at its capture time.
3. `fthrust-core::finalize::plan` builds one filter graph: every layer
   (`movie=` source, fit with transparent padding or fill with a centre crop,
   opacity as alpha) in list order, then the camera letterboxed in black on
   top. Pixel geometry is the reference's (`int()` truncation, 64×48
   minimum, even sizes).
4. The plan 6c runner re-encodes the video once (High quality), copies every
   audio stream with its title, keeps the container tags, and stages the
   result in `.fthr-finalize-*` beside the clip; it is renamed over the clip
   and the audio manifest is rebound to the new file.

Any failure keeps the base clip and records a warning; the last one shows as
`CLIP SAVED WITH WARNING`. A failed camera burn also shows `CAMERA OVERLAY
FAILED`. Clips finalize one at a time in save order; quitting cancels the
running one.

## The webcam

`fthrust-media::camera` reads the selected V4L2 camera through libavdevice
(MJPEG requested; raw formats are encoded to MJPEG) into a 90 s ring stamped
at capture. Devices are listed with their V4L2 names. `services::camera`
scans at start and on Refresh, opens the selected camera while the overlay is
on, turns the overlay off with `CAMERA UNAVAILABLE` (action `OPEN CAMERA
SETTINGS`) when it cannot open, and gives the Visuals placement box a live
frame at 10 fps.

## Gary mode

A PipeWire capture stream on the selected microphone (`services::mic_level`)
measures `clamp(rms × 4)` of channel 0 after the mic gain; it feeds the Audio
page meter and Gary, and runs only while either needs it. Gary
(`overlay::gary`) ticks every 33 ms on its own thread: intensity steps up by
0.18 and down by 0.065 towards the level between the MIN and MAX handles;
the picture grows from 55 % to 100 % of a 58 % × 76 % box and fades in; one
appearance in ten shows the cat. It draws on a click-through layer-shell
surface covering the output (`notification_monitor`, else the compositor's
choice) that exists only while it shows. Without layer-shell Gary stays off.

## Pickers

Every BROWSE / Choose / Import button opens the desktop FileChooser portal
(`services::browse`) and applies the choice as the reference did: clips
storage folder (prepared with its five subfolders; the recording folder
follows when it was derived), recording folder (linked into the library when
outside it), import folders, the Gary image, image layers, the preview
background, theme archives (after confirmation), fonts, icons and sounds.

## Differences from the reference

See `CHANGES.md`: one re-encode instead of one per overlay; camera frames at
their real times; unreadable layers dropped instead of failing every layer;
libavdevice instead of OpenCV; portal pickers; icons resized without the crop
dialog; picker failures shown instead of printed.

## Tests

- `cargo test -p fthrust-core --test finalize_model`
- `cargo test -p fthrust-media --test camera`, `--test export_run` (rewrites)
- `cargo test -p fthrust-ui --test finalize --test gary --test browse`
- `tools/uidrive/bench.sh run tools/uidrive/scenarios/overlays.sh --settings tools/uidrive/overlays-settings.json`
