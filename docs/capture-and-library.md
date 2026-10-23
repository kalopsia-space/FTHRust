# Capture and library

How the UI runs the engine, delivers hotkeys, saves clips, screenshots and
recordings, and shows the capture card. Paths are the Linux ones; nothing
here applies to Windows.

## The engine

`fthrust-ui` looks for `fthrust-engine` next to its own executable, then in
`target/{debug,release}` when run from a checkout. It starts the daemon with
`--socket $XDG_RUNTIME_DIR/fthrust/engine.sock`, reads `FTHR_STARTUP_ERROR` /
`FTHR_STARTUP_WARNING` lines from its stderr (mirrored to
`<diagnostics>/engine.log`), connects within 9 s and sends `Configure`.
A supervisor (`services/engine.rs`) owns the lifecycle: launch generations,
the 30 s apply timeout on restarts, health from the engine's status frames,
automatic restarts with the core recovery tracker's backoff, and the status
strip text.

| Status | When |
| --- | --- |
| `STARTING CAPTURE` | launch or restart in progress |
| `CAPTURING` | healthy |
| `CAPTURE DEGRADED` / `CAPTURE DEGRADED — DARK / UNIFORM` | frames arrive but look wrong |
| `CAPTURE STALLED` / `RECOVERING CAPTURE` | no frames; the engine or the supervisor is recovering |
| `CAPTURE FAILED` / `ENGINE STOPPED` / `NO ENGINE` / `DISCONNECTED` | the daemon is gone; the error bar offers RESTART CAPTURE |
| `APPLYING SETTINGS` / `RESTARTING ENGINE` / `SETTINGS APPLY TIMED OUT` | a settings change is being applied |
| `PAUSED — GAME UNFOCUSED` | focus pause (see below) |
| `SAVING` / `SAVE SLOW…` / `SAVE FAILED` | a replay save |
| `PREPARING RECORDING QUALITY` / `STARTING RECORDING` / `FINALIZING RECORDING` / `RECORDING SAVED` / `RECORDING FAILED` | manual recording |

Settings that change the capture configuration (clip length, resolution,
bitrate, framerate, audio sources, source window) restart the engine when
applied; the same restart serves `restart` and `restart-capture` error-bar
actions.

## Hotkeys

Hotkeys reach the running instance three ways; all of them end in the same
`services::on_hotkey` dispatcher.

- **Private socket.** `$XDG_RUNTIME_DIR/fthrust/hotkey.sock` (mode 0600) accepts
  one action name per connection (`save_clip`, `save_screenshot`,
  `start_recording`, `stop_recording`, `confirm_game_detection`,
  `dismiss_game_detection`, `save_custom_clip_<seconds>`). A silent or junk
  client is dropped after 500 ms. The client is the app itself:
  `fthrust --hotkey save_clip`.
- **Hyprland.** On Hyprland the app writes `~/.config/hypr/fthr-hotkeys.conf`
  with one `bind = …, exec, "<fthrust>" --hotkey <action>` per bound action,
  adds `source = ~/.config/hypr/fthr-hotkeys.conf` to `hyprland.conf` once,
  and runs `hyprctl reload` when the file changed. The Hotkeys popup shows the
  instructions and a COPY LINE button for a manual bind.
- **GlobalShortcuts portal.** On desktops with the portal the app binds the
  same actions through `org.freedesktop.portal.GlobalShortcuts`; whatever the
  desktop keeps is adopted into the hotkey settings and a `DESKTOP KEPT ITS
  SHORTCUT` warning lists the differences. EDIT IN DESKTOP SETTINGS opens the
  portal's configuration dialog.

## Saves, screenshots and recordings

- **Replay clips** go to `<clips>/Desktop/desktop_clip_from_<date>.mp4` or
  `<clips>/<game>/<game>_clip_from_<date>.mp4`, never overwriting. Saves are
  refused while capture is unhealthy, with the health detail in the error bar.
  `SAVE SLOW…` appears after 1 s without an acknowledgement and `SAVE FAILED`
  after 60 s.
- **Screenshots** use `org.freedesktop.portal.Screenshot` (non-interactive)
  and land in `<clips>/Screenshots/screenshot_from_<date>.png`. One request runs
  at a time with up to nine queued; the PNG is validated before it is published.
- **Manual recordings** need 512 MiB free and a path under 255 characters,
  restart the engine with the recording quality profile when it differs from
  the clip profile, and are written as `FTHR_recording_<date>.mp4` in the
  recording folder. The engine must acknowledge the start within 10 s. On
  exit a running recording is stopped and finalised before the engine is
  shut down.

## Game detection and focus pause

The compositor's `zwlr_foreign_toplevel_management_v1` list feeds the Source
popup's window list, game detection (fullscreen toplevels or custom rules,
minus the blacklist; `prompt` mode shows a card for 15 s, `auto` switches) and
focus pause (`anticheat_detection_enabled` with a window source: capture pauses
while another window is active). Without the protocol the Game dropdown reads
`Game detection unavailable` and a `LIMITED FEATURE SUPPORT` warning explains
why; hotkeys and capture are unaffected.

## The capture card

Cards are drawn on a `zwlr_layer_shell_v1` overlay surface (top-right, 16 px
margins, highest-refresh output unless `notification_monitor` names one) by
egui over wgpu, with the reference's timeline: 450 ms slide-in, 2.4 s hold with
a draining progress bar and a shimmer, 350 ms slide-out. Without layer-shell a
desktop notification carries the headline and detail. `capture_card_enabled`
off keeps the sounds only. Sounds are the six bundled cues (or the theme's
custom ones), played through a PipeWire playback stream at the cue volume
times the master volume.

## Live tests

These need a Wayland session and are ignored by default:

```sh
cargo test -p fthrust-wayland --test layer_shell_live -- --ignored
cargo test -p fthrust-ui --test overlay_card overlay_live_shows_card -- --ignored
cargo test -p fthrust-ui --test services_screenshot screenshot_live_portal -- --ignored
```

The synthetic engine round trip (`services_engine`) runs whenever
`target/<profile>/fthrust-engine` exists; CI builds it before the tests.
