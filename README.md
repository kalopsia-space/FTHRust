# FTHRust

FTHRust is a from-scratch Rust rewrite of the Linux side of FTHR Clips, a
local-first instant-replay recorder. It captures the screen into a rolling
buffer, a hotkey saves the last N seconds as a clip, and a built-in library and
editor let the user trim, export and optionally upload clips. No account, no
cloud, no telemetry.

Design: [docs/superpowers/specs/2026-10-22-fthrust-design.md](docs/superpowers/specs/2026-10-22-fthrust-design.md)

Derived from FTHR Clips by FTHR-Community, GPL-3.0-only.

## Running the UI

```sh
cargo run -p fthrust-ui --release        # the FTHR Clips window (Wayland)
FTHRUST_ASSETS=/path/to/assets cargo run -p fthrust-ui   # assets elsewhere than ./assets
```

`fthrust --version` prints the version. The UI needs a Wayland session; the
engine is started by the UI (plan 5) or by hand as below.

The capture side of the UI (engine supervision, hotkeys, saves, screenshots,
recordings, game detection, the capture card) is described in
[docs/capture-and-library.md](docs/capture-and-library.md).
The editor's playback pipeline (decoders, mixer, audio clock, sync, the
seek proxy) is described in [docs/playback.md](docs/playback.md).

## Running the engine by hand

The capture daemon is a separate binary. For debugging it can be driven
without the UI:

```sh
cargo run -p fthrust-engine -- --socket "$XDG_RUNTIME_DIR/fthrust/engine.sock"
cargo run -p fthrust-engine -- --list-playback-devices
FTHR_CAPTURE_BACKEND=wlr cargo run -p fthrust-engine -- --socket /tmp/engine.sock
```

It prints `FTHRUST_ENGINE_READY <socket>` and then speaks the protocol in
`docs/engine-protocol.md` (length-prefixed JSON). It exits when its stdin
is closed, so run it from a shell that keeps stdin open. The interactive
portal path can be exercised with
`FTHRUST_PORTAL_TEST=1 cargo test -p fthrust-portal --test portal_live`.
