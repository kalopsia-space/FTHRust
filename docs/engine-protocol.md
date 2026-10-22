# fthrust-engine protocol

The capture daemon (`fthrust-engine`) and the UI (`fthrust`) talk over a
Unix domain socket. Everything here is defined by the `fthrust-ipc` crate;
this page is the human-readable summary.

## Transport

- Socket: `$XDG_RUNTIME_DIR/fthrust/engine.sock` (fallback
  `~/.fthr/run/engine.sock`), directory mode 0700, socket mode 0600. The UI
  starts the engine with `fthrust-engine --socket <path>` and connects
  after the engine prints `FTHRUST_ENGINE_READY <path>` on stdout.
- Framing: every message is a little-endian `u32` byte count followed by a
  JSON document (`serde_json` of the `Command` or `Event` enum). Frames
  above 16 MiB are refused.
- One client at a time. A client that disconnects does not stop capture;
  the next client gets `Hello`, the startup notices, and a `Status` whose
  `last_save` still holds the most recent save outcome.
- `PROTOCOL_VERSION = 1` is carried in `Hello`; a mismatch closes the
  connection on the client side.

## Lifecycle

1. The UI spawns the engine with its stdin piped (the engine exits when
   stdin reaches EOF: that is the parent watch) and reads stdout until the
   ready line.
2. The UI connects and sends `Configure(EngineConfig)`. The engine starts
   audio and the first capture generation and answers `Configured { warnings }`,
   or `StartupError { code, detail }` followed by exit code 1.
3. The engine pushes `Status` whenever something changes and at least every
   500 ms (`Heartbeat` when nothing changed).
4. `Shutdown`, stdin EOF, SIGTERM or SIGINT end the process with exit
   code 0 after a bounded audio and capture stop.

## Commands

| Command | Reply |
|---|---|
| `Configure(EngineConfig)` | `Configured { warnings }` (one line per clamped field), or `StartupError` |
| `SaveClip { request_id, path, seconds }` | `SaveStarted`, `SaveProgress` (per MiB), `SaveResult { result }` |
| `StartRecording { request_id, path }` | `RecordingStarted` or `RecordingError` |
| `StopRecording { request_id }` | `RecordingStopped { path, bytes }` or `RecordingError` |
| `Pause` / `Resume` | a `Status` with the `PAUSED` flag |
| `GetStatus` | `Status` |
| `Shutdown` | process exit 0 |

`EngineConfig` is the UI's `CaptureConfig` plus `max_buffer_mb` (ring byte
budget, 0 = none) and `backend_override` (`FTHR_CAPTURE_BACKEND` is the
default when the field is `None`).

Example frame (header omitted):

```json
{"SaveClip":{"request_id":7,"path":"/home/me/Videos/FTHR/desktop_clip_from_22Oct2026_21-15-03.mp4","seconds":30}}
```

## Status

`StatusSnapshot` keeps the reference engine's fields and semantics:
`frames_captured`, `encoder_open` (the reference's `nvenc_active`),
`active_codec`, `health_flags` (ACTIVE 1, RECOVERING 2, BACKEND_FAILED 4,
CONTENT_SUSPECT 8, PAUSED 16), `capture_generation`, `encoder_generation`,
the content watchdog metrics, `capture_failure_detail` (with
BACKEND_FAILED), `recovery_reason` (with RECOVERING), `paused`,
`recording`, `backend`, `selected_output`, native size, `audio` source
states and `last_save`.

## Startup codes

Printed on stderr as `FTHR_STARTUP_ERROR: <CODE>: <detail>` /
`FTHR_STARTUP_WARNING: <CODE>: <detail>` and sent as events.

| Code | Kind | Meaning |
|---|---|---|
| `WAYLAND_UNAVAILABLE` | error | no `WAYLAND_DISPLAY`, or a Wayland backend was forced without one |
| `CAPTURE_BACKEND_UNAVAILABLE` | error | every candidate backend failed and the retry budget is spent |
| `ENCODER_UNAVAILABLE` | error | no candidate encoder opened |
| `CAPTURE_DECLINED` | error | the ScreenCast picker was dismissed |
| `DESKTOP_AUDIO_UNAVAILABLE` | warning | PipeWire audio could not be started; capture continues video-only |

## Exit codes

0 after `Shutdown`, stdin EOF or a signal; 1 after a startup error; 2 for a
usage error.

## Environment

| Variable | Effect |
|---|---|
| `FTHR_CAPTURE_BACKEND` | `wlr`, `ext`, `portal` or `synthetic`: force one backend, no fallback |
| `FTHR_VAAPI_DEVICE` | first render node to try for VA-API |
| `FTHRUST_SYNTHETIC_BACKEND=1` | allow the synthetic test backend (`FTHRUST_SYNTHETIC_SIZE=WxH`) |
| `FTHRUST_DMABUF` | reserved for the DMA-BUF path (not implemented yet; the shm path is used) |
| `FTHRUST_PORTAL_TEST=1` | run the interactive portal test |
| `FTHRUST_LIVE_SOFTWARE=1` | let the live qualification test use a software encoder |
| `FTHRUST_LOG` | tracing filter for the engine's stderr log |

## Listings

`fthrust-engine --list-playback-devices` and `--list-microphones` print one
JSON line in the reference's shape
(`{"schema_version":1,"microphones":[{"endpoint_id","display_name","device_state":1,"is_default"}]}`)
and exit 0, or `{"schema_version":1,"error":"..."}` on stderr with exit 1.
