# Rust crate research: Wayland instant-replay recorder (egui/eframe UI)

Date: 2026-10-22. Versions verified with `cargo search` + crates.io API (max_stable_version); maintenance from
GitHub `pushed_at`. Local reference box: Arch, FFmpeg 9.0.2, libva 2.24.1, PipeWire 1.6.9, GStreamer 1.28.8,
wayland-protocols 1.49 (ships ext-image-copy-capture-v1 / ext-image-capture-source-v1 / ext-foreign-toplevel-list-v1,
all interface version 1), Mesa 26.2.5 radeonsi VAAPI (H264/HEVC/AV1 EncSlice available).

Legend: Verdict = USE / OK (acceptable) / AVOID / WATCH (promising, immature or stale).

---

## 1. Wayland client

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| wayland-client | 0.31.15 (2026-07) | Smithay, active | USE | Features: `dlopen` (recommended for AppImage), `system`, `libwayland_1_23`. |
| wayland-backend | 0.3.17 | active | USE | Transitive; enable `client_system` + `dlopen`. |
| wayland-protocols | 0.32.13 (2026-06) | active | USE | Features `client`, `staging`, `unstable`. Module `ext::image_copy_capture`, `ext::image_capture_source`, `ext::foreign_toplevel_list` are present (all under `staging`). `xdg::xdg_output` is present (under `unstable`). |
| wayland-protocols-wlr | 0.3.12 (2026-03) | active | USE | `screencopy` (zwlr_screencopy_v1), `export_dmabuf`, `layer_shell`, `foreign_toplevel`, `output_management` all generated. |
| wayland-protocols-misc | 0.3.12 | active | OK | Only needed for gtk-primary-selection / server-decoration / input-method; not needed here. |
| wayland-protocols-plasma | 0.3.12 | active | OK | KDE-specific (not needed unless you want KWin-only paths). |
| wayland-scanner | 0.31.11 | active | USE (fallback) | Only needed if you want a protocol version newer than the crate ships or a vendor XML (e.g. hyprland-protocols); generate from local XML in `build.rs`. |
| smithay-client-toolkit | 0.21.1 (2026-07) | active | USE | Registry/output/shm/dmabuf helpers and `shell::wlr_layer::{LayerShell, LayerSurface, Anchor, Layer}`. NOTE: winit 0.30.13 pins sctk 0.19.2 internally; using sctk 0.21 in your own code compiles two sctk versions (fine, just binary size) -- they share wayland-client 0.31 so the `wl_display` connection can be shared via `winit`'s `WaylandDisplayHandle` + `wayland_client::Connection::from_backend`. |

Verdict: **no wayland-scanner needed** for any protocol you listed; all five are generated bindings. Gotcha:
ext-image-copy-capture requires compositor support (wlroots 0.19+/Sway 1.11, Hyprland, KWin 6.3+, COSMIC, niri;
GNOME/Mutter does NOT expose it or wlr-screencopy -- GNOME users must go through the portal/PipeWire path).

## 2. PipeWire + xdg-desktop-portal

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| pipewire (pipewire-rs) | 0.10.1 (2026-08) | freedesktop, active | USE | Features `v1_0_0`, `v1_2_0` gate newer APIs. Bindings to libpipewire (C); needs libpipewire-0.3 at runtime. Stream API: `pw::stream::Stream` + `StreamListener` with `process` callback; buffers expose `libspa::buffer::DataType::{DmaBuf, MemFd, MemPtr}`. Still "0.x": API churn between 0.8 -> 0.9 -> 0.10 was significant. |
| libspa / pipewire-sys | 0.10.1 | active | USE | Transitive. You will write raw SPA pod format negotiation (video/raw + DMA-BUF modifier lists) -- the most verbose part. |
| pipewire-native | 0.2.0 | WATCH | WATCH | Pure-Rust PipeWire client (Asymptotic). Too young for production; keep an eye on it for dropping the C dependency. |
| ashpd | 0.13.13 (2026-07) | bilelmoussaoui, active | USE | Portals: `screencast`, `screenshot` (`Screenshot::request`, interactive/modal, `PickColor`), `remote_desktop`, `global_shortcuts` (`bind_shortcuts`, `list_shortcuts`, `configure_shortcuts`, `Activated`/`Deactivated`/`ShortcutsChanged` signals) -- all confirmed present as feature flags and modules. Default feature `tokio` (= `zbus/tokio`); alternative `async-io`. Also `wayland` feature (exports `WindowIdentifier` from wl_surface), `raw_handle` (from raw-window-handle, works with eframe), `pipewire` (opens the screencast fd as a pw stream). |

Gotchas:
- ashpd 0.13.13 depends on **zbus ^5.13** (master still on 5.12 as of today) -- NOT zbus 6. See section 3.
- ashpd needs a running async runtime; with `tokio` feature you must call it from inside a tokio runtime (egui runs its own loop -> spawn a tokio runtime thread and bridge with channels).
- GlobalShortcuts portal requires xdg-desktop-portal >= 1.17 and a backend that implements it (GNOME 45+, KDE 5.27/6, Hyprland portal, COSMIC); wlr backend does NOT implement it.
- ScreenCast portal gives a PipeWire node id + fd; frame DMA-BUF negotiation then happens in pipewire-rs.

## 3. D-Bus

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| zbus | 6.0.0 (2026-10-16) | active | WATCH | Brand new major. Breaking: `async-io` feature REPLACED by `default-rt` (zruntime, per-thread runtime); blocking API removed; VSOCK dropped; bitflags types; D-Bus API gated behind `comms`. Features now: `default-rt` (default), `tokio`, `tokio-multithread`. |
| zbus | 5.19.0 (2026-08) | maintained | USE (today) | Last 5.x. Features `tokio` or `async-io`. Choose this to match ashpd 0.13.x and avoid two zbus copies in the build. Move to 6 when ashpd 0.14 lands. |

Recommendation: single tokio runtime for zbus/ashpd/russh/reqwest (`zbus = { version = "5", default-features = false, features = ["tokio"] }`); avoid mixing async-io + tokio.

## 4. FFmpeg bindings / encode pipeline

| Crate | Version | Maintenance | FFmpeg headers | Verdict | Notes |
|---|---|---|---|---|---|
| ffmpeg-sys-next | 9.0.1 (2026-10-09) | zmwangx, "maintenance mode" but active | build.rs detects up to `ffmpeg_9_0` (avcodec 63.1), plus 8.0/8.1 | USE | Bindgens `libavutil/hwcontext.h`, `hwcontext_drm.h` (if present), `hwcontext_vaapi` via stubs; `avfilter` feature on by default. Links system FFmpeg via pkg-config; `build` feature compiles FFmpeg from source (slow). |
| ffmpeg-next | 9.0.0 (2026-08) | same | FFmpeg 9.0 / 8.x | USE (with raw `ffi` escape hatch) | Safe wrapper: format/codec/filter/swscale/swresample. Unreleased: `Rc`->`Arc` Send-soundness fix (breaking for `wrap`). Wrapper has NO safe hwcontext/hwframes API -- you'll do `av_hwdevice_ctx_create`, `av_hwframe_ctx_alloc`, `av_hwframe_map` via `ffmpeg_next::ffi`. |
| rsmpeg | 0.18.0+ffmpeg.8.0 (2025-08) | 14 months no push | up to 8.0 only | AVOID (for FFmpeg 9) | Nicer, more complete wrapper, but no FFmpeg 8.1/9 support and stale. |
| rusty_ffmpeg | 0.17.0+ffmpeg.8.1 (2026-04) | CCExtractor, slow | up to 8.1 | OK (sys only) | Sys-level only; no 9.0 yet. |
| gstreamer / -app / -video / -pbutils | 0.25.4 (2026-09) | very active | GStreamer 1.16..1.30 features | STRONG ALTERNATIVE | `pipewiresrc` (gst-plugin-pipewire, ships with PipeWire), `va*` encoders (vah264enc/vah265enc/vaav1enc/vapostproc in gst-plugins-bad), `fdkaacenc`/`avenc_aac`/`voaacenc`, `mp4mux`/`qtmux` with `fragment-duration` and `isofmp4mux` (gst-plugins-rs), `splitmuxsink`. DMA-BUF flows end-to-end (`video/x-raw(memory:DMABuf)`) without you writing hwframe code. |

Specific answers:
- FFmpeg 8.x/9.x headers: ffmpeg-sys-next 9.0.1 supports 8.0, 8.1 and 9.0 (version table in build.rs). rsmpeg: 8.0 max. Arch is already on 9.0.2, Fedora 44/Ubuntu 26.04 are on 8.x -> you must support both.
- VAAPI from Rust via ffmpeg-next: all via `ffi`: create `AV_HWDEVICE_TYPE_VAAPI` (optionally from a DRM render node via `AV_HWDEVICE_TYPE_DRM` then `av_hwdevice_ctx_create_derived`), encoder `h264_vaapi`/`hevc_vaapi`/`av1_vaapi`, frames must be `AV_PIX_FMT_VAAPI` with `hw_frames_ctx` set. Expect ~150-300 lines of unsafe glue.
- AAC: `aac` (native, fine at 160-192k), `libfdk_aac` only if system FFmpeg was built nonfree (Arch no, Debian no) -> use native `aac`; or encode audio with the `opus` route only if you target MKV/WebM.
- Fragmented MP4: `mov` muxer with `movflags=frag_keyframe+empty_moov+default_base_moof` (+`faststart` not usable with fragments). For instant replay keep a ring of encoded packets (GOP-aligned) in memory and mux on save -- this is what GPU Screen Recorder / Replay Sorcery do; it avoids fragment muxing entirely.
- libavfilter: available (`avfilter` default feature in sys; `ffmpeg_next::filter` safe API), including `scale_vaapi`/`hwupload`/`hwmap` filters.
- Local check: `vainfo` on this machine shows H264/HEVC EncSlice via Mesa radeonsi.

Recommendation: FFmpeg (ffmpeg-next 9 + raw ffi for hw paths) if you want one binary with tight control and no plugin-discovery runtime; **GStreamer if you want DMA-BUF zero-copy with minimal unsafe and will ship a Flatpak**. For an AppImage, GStreamer is painful (plugin registry, dozens of .so + GLib stack, `GST_PLUGIN_PATH` relocation), FFmpeg bundles as ~8 shared libs. Tradeoff summary: FFmpeg = more unsafe, simpler packaging; GStreamer = less unsafe, heavier packaging, pipeline debugging via GST_DEBUG is excellent.

## 5. DMA-BUF / GPU zero-copy

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| drm | 0.15.0 (2026-03) | Smithay, active | USE | Render node open, `drm::buffer`, PRIME fd import/export, `drm-fourcc` for formats. |
| drm-fourcc | 2.2.0 | active | USE | Fourcc + modifier enums. |
| gbm | 0.18.0 (2024-12; repo pushed 2025-06) | slow | OK | Allocate `gbm_bo` with explicit modifiers for ext-image-copy-capture dmabuf buffers (`zwp_linux_dmabuf_v1` -> `wl_buffer`). Needs libgbm. |
| dma-buf | 0.5.0 | small | OK | Sync ioctl helpers (DMA_BUF_IOCTL_SYNC); or do ioctl yourself via `rustix`. |
| libva | 0.1.5 (2026-10-17) | fork of cros-libva by mlm-games; features `dlopen` (default), `linked`, `vendored` | WATCH/USE | Safe VA-API surface/context/buffer API incl. surface import from DRM PRIME. Only path if you want to drive VA-API encode WITHOUT FFmpeg/GStreamer (large effort: write H264 param structs yourself). |
| cros-libva | 0.0.13 (2024-12) | crate stale; repo pushed 2026-09 but unreleased | AVOID | Use `libva` fork or git dep. |
| va-rs | -- | does not exist on crates.io | -- | |
| khronos-egl / wayland-egl | 6.0.0 / 0.32.11 | active | OK | Only if you want EGL import of dmabuf for GPU-side scaling/compositing (overlay webcam, cursor) before encode. |

Zero-copy path (no CPU copy):
1. ext-image-copy-capture / wlr-screencopy: query `dmabuf_device` + `dmabuf_format` (ext) or `linux_dmabuf` event (wlr v3); allocate a gbm bo with a supported modifier; wrap as wl_buffer via zwp_linux_dmabuf_v1; `attach_buffer` + `capture`; on `ready` you own a DMA-BUF fd.
2. FFmpeg: build an `AVFrame` of `AV_PIX_FMT_DRM_PRIME` with `AVDRMFrameDescriptor` (fd, layer, plane offsets/pitches, modifier), then `av_hwframe_map(vaapi_frame, drm_frame, AV_HWFRAME_MAP_READ)` into a VAAPI hwframes ctx (or `hwmap` filter with `derive_device=vaapi`). Encoder consumes `AV_PIX_FMT_VAAPI` frame. If the capture format is BGRA/XRGB you need `scale_vaapi` to NV12 (GPU VPP) first -- some AMD VAAPI stacks lack RGB->NV12 VPP for certain modifiers (compression) -> request `DRM_FORMAT_MOD_LINEAR` or a non-compressed modifier when allocating.
3. PipeWire portal path: negotiate `SPA_DATA_DmaBuf` with modifier list in the format pod; same DRM_PRIME import.
Realistic fallback: wl_shm (BGRA) -> CPU `AVFrame` -> `hwupload` (sw->vaapi, ~8 ms at 4K) or software x264 `veryfast`. Keep this path working; it's what you get on GNOME via portal MemFd buffers and on hosts without PRIME import (NVIDIA proprietary: no VAAPI at all -> `h264_nvenc` via CUDA or software).

## 6. Audio capture

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| pipewire (stream, audio) | 0.10.1 | active | USE | Capture stream with `PW_KEY_STREAM_CAPTURE_SINK=true` targets the default sink's monitor; second stream for mic (`target.object`); per-buffer PTS from `pw_time`/`SPA_META_Header` with clock rate -> monotonic timestamps directly comparable to video frame timestamps (CLOCK_MONOTONIC). |
| libpulse-binding | 2.30.1 (2025-04) | jnqnfe, slow but stable | OK | Works through pipewire-pulse; monitor source `<sink>.monitor`; latency timing via `stream.get_time()`/`get_latency()`, less precise. Requires libpulse at runtime (present on almost all desktops). |
| libpulse-simple-binding | 2.29.0 | same | AVOID | Blocking simple API, no timestamps. |
| cpal | 0.18.2 | active | AVOID | ALSA backend only on Linux, no monitor capture. |

Recommendation: native PipeWire streams (one for sink monitor, one for mic), both with `SPA_META_Header` timestamps; resample/mix in-process (or encode as two AAC tracks). Fallback to libpulse-binding only if PipeWire absent (rare in 2026).

## 7. egui / eframe

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| egui / eframe / egui-wgpu / egui-winit / egui_extras | 0.37.0 (2026-10-15) | very active | USE | eframe deps: winit ^0.30.13, wgpu ^30.0, glow 0.17. eframe features: `wgpu`, `wayland`, `x11`, `persistence`, `system_fonts`, `default_fonts` (no longer default in egui!), `clipboard` (now optional). |
| winit | 0.30.13 stable (2026-03); 0.31.0-beta.3 (2026-09) | active | USE 0.30 via eframe | Default features include `wayland`, `wayland-dlopen`, `wayland-csd-adwaita` (CSD fallback drawn by sctk-adwaita when compositor lacks xdg-decoration, i.e. GNOME). Fractional scaling: supported via wp-fractional-scale-v1 (per-monitor scale events). 0.31 will break eframe API (not yet adopted). |
| wgpu | 30.0.1 (2026-08) | active | USE | Vulkan backend on Wayland; GL fallback. |
| resvg / usvg | 0.48.1 (2026-08) | active | USE | egui_extras 0.37 pins `resvg ^0.45.1` for the `svg` loader -> if you also use resvg directly, pin 0.45 or accept two copies. |
| egui-notify | 0.23.0 (2026-09) | active | USE | In-window toasts. |
| egui-toast | 0.22.0 | ok | OK | Alternative. |
| egui-video | 0.9.0 (2024-10) | stale | AVOID | ffmpeg-based player widget; roll your own texture upload. |

Answers:
- Wayland status: eframe 0.37 "keeps running on Wayland when frame callbacks stop" (#8631), activation-token handoff (#8406), PRIMARY selection. Window positioning (`with_position`), `with_always_on_top`/`set_window_level` and `set_cursor_position` are **unsupported on Wayland** by winit (silently ignored). Frameless (`with_decorations(false)`) works. Transparent works with wgpu (`with_transparent(true)`, alpha surface).
- Custom fonts: `Context::set_fonts` / `add_font`; 0.37 adds `add_font_provider` + `egui_system_font_provider` (runtime glyph fallback incl. emoji). Enable `default_fonts` explicitly now.
- SVG icons: `egui_extras` features `svg` / `svg_images` / `svg_text` (resvg); `install_image_loaders(ctx)`; `Image::new(include_image!("icon.svg"))`. Rasterized once per size, cached.
- 4K60 video texture upload: `TextureHandle::set` with `ColorImage` means a CPU RGBA copy (33 MB/frame, 2 GB/s at 60 fps) plus `queue.write_texture` -- feasible on desktop but wasteful. Better: `egui_wgpu::Renderer::register_native_texture` / `update_egui_texture_from_wgpu_texture` and write the frame straight into a wgpu texture yourself (NV12->RGB in a compute/fragment shader, or `egui_wgpu::CallbackTrait` for a custom draw). For previews, downscale to 1080p before upload. DMA-BUF -> wgpu import is NOT available in wgpu 30 public API (needs Vulkan external memory via `wgpu-hal`), so the preview will always be a CPU/VAAPI readback unless you go hal.
- Virtual scrolling: `ScrollArea::show_rows(ui, row_height, total_rows, |ui, range| ...)` and `show_viewport` both exist in 0.37.
- Notifications / multiple windows: egui viewports (`ctx.show_viewport_deferred/immediate`) create extra winit windows; on Wayland they are normal xdg_toplevels (no position, no always-on-top). Good for a settings/popout window, NOT for an overlay HUD.
- Transparent always-on-top overlay on Wayland: **not through winit** (no layer-shell support; still none in 0.31 beta). Options: (a) separate process/thread with smithay-client-toolkit 0.21 `wlr_layer::LayerShell` + wgpu via raw-window-handle + egui-wgpu/egui-winit-less integration (feed input yourself; ~500 lines), (b) crates `layershellev` 0.19.1 (winit-like event loop for layer-shell, used by iced_layershell) -- no egui glue exists, (c) `lockrs`/`egui-pop-wayland` show egui-on-raw-Wayland is doable. Layer-shell is unsupported on GNOME; fall back to a frameless toplevel + compositor "keep above" rule, or desktop notifications via `notify-rust`/portal.

## 8. Webcam

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| nokhwa | 0.10.11 (2026-05) | repo moved to l1npengfih/nokhwa, active | USE | `input-v4l` feature; MJPEG/YUYV decode (`decoding`); `output-wgpu`. Dev branch 0.11 has churned for years -- stay on 0.10. |
| v4l | 0.14.0 (2023-05; repo 2024-10) | stale | OK | Direct V4L2 mmap/userptr streaming, good if you only need raw frames and control. |
| rscam | 0.5.5 | dead | AVOID | |
| pipewire (camera portal) | 0.10.1 + ashpd `camera` | active | ALT | Required inside Flatpak sandbox; also works on bare metal (`libcamera` devices appear). |

Recommendation: nokhwa 0.10 (input-v4l) for simplicity; raw `v4l` if you need buffer-level control (DMA-BUF export for GPU compose is possible via V4L2_MEMORY_DMABUF but neither crate wraps it nicely).

## 9. Shared memory / process model

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| memmap2 | 0.9.12 (2026-10) | active | USE | `MmapMut` over a `memfd`/`/dev/shm` file; pair with `rustix::fs::memfd_create` + fd passing over a UNIX socket (`sendmsg` SCM_RIGHTS via `rustix` or `sendfd`). |
| shared_memory | 0.12.4 | moderate | OK | Named shm with an easier API; no sync primitives. |
| interprocess | 2.4.6 (2026-10) | active | USE | Local sockets (abstract-namespace on Linux) for control channel/single instance. |

Recommendation: keep **single process** (capture + encode threads + egui thread) unless you need the UI to crash independently of the recorder. If you split, put the recorder in a daemon (systemd --user unit) and talk over a UNIX socket; share frame ring via memfd + `#[repr(C)]` structs with `AtomicU64` seqlock. Note: the portal ScreenCast session is tied to the D-Bus connection owner, so the daemon must own it.

## 10. Upload / secrets / hardware ID

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| russh | 0.64.1 (2026-10) | warp-tech, very active | USE | Pure Rust SSH (tokio). |
| russh-sftp | 3.0.1 (2026-09) | active | USE | SFTP client over russh channels; supports streaming writes. |
| ssh2 | 0.9.6 (repo rust-lang/ssh2-rs, pushed 2026-10) | maintained | OK | libssh2 + OpenSSL C deps -- hurts AppImage/static builds; blocking API. |
| reqwest | 0.13.5 (2026-09) | active | USE | `multipart` + `stream` features; use `rustls-tls` (avoid OpenSSL in AppImage). |
| keyring | 4.2.0 (2026-08) | active, restructured | USE (with store crates) | v4 = thin CLI/sample; apps depend on `keyring-core` 1.0.1 + a store: `zbus-secret-service-keyring-store` 1.0.1 (async, zbus; matches your zbus choice) or `dbus-secret-service-keyring-store` 1.0.1 (libdbus C) or `linux-keyutils-keyring-store` 1.0.0 (kernel keyring, no persistence across reboot). Secret Service needs gnome-keyring/kwallet6 running -> have an encrypted-file fallback. |
| secret-service | 5.2.0 | active | OK | Direct Secret Service API if you skip keyring-core. |
| machine-uid | 0.6.0 (2026-05) | active | USE | Reads `/etc/machine-id` (falls back to `/var/lib/dbus/machine-id`). Changes on reinstall/cloned images; hash it with a salt before sending. |

## 11. Global hotkeys on Wayland

Confirmed: there is **no** Wayland protocol for client global hotkeys; compositor-agnostic path is the
`org.freedesktop.portal.GlobalShortcuts` portal (ashpd `global_shortcuts`, see section 2). `global-hotkey`
0.9.0 README: "Linux (X11 only)". Hyprland/Sway can bind `exec` commands in their config that call your CLI/socket.

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| evdev | 0.13.2 (repo emberian/evdev, 2026-05) | active | OK (fallback) | Read `/dev/input/event*` directly; requires user in `input` group or a udev rule (security: this is a keylogger capability; Flatpak sandbox forbids). `tokio` feature for async streams. Grab nothing; just observe EV_KEY and match chords. Keymap must be derived yourself (XKB via `xkbcommon` crate) if you want layout-aware names. |
| ashpd global_shortcuts | 0.13.13 | active | USE | Primary. |

## 12. Image / thumbnails

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| image | 0.25.10 | active | USE | PNG/JPEG/WebP encode; `jpeg` via zune-jpeg is fast. |
| fast_image_resize | 6.1.0 (2026-07) | active | USE | SIMD resize (AVX2/NEON); 4K->360p thumbnail in ~2-3 ms. |
| resvg / usvg / tiny-skia | 0.48.1 / 0.48.1 / 0.12.0 | active | USE | See section 7 (egui_extras pins resvg 0.45). |

Thumbnails from the ring: decode nothing -- grab the last captured raw frame (or a VAAPI readback via `av_hwframe_transfer_data`) and downscale.

## 13. Single instance + activation

| Crate | Version | Maintenance | Verdict | Notes |
|---|---|---|---|---|
| single-instance | 0.3.3 (2021) | dead | AVOID | Uses abstract unix socket but no message passing. |
| interprocess | 2.4.6 | active | USE | `LocalSocketListener` on `@yourapp-$UID` (abstract namespace) -> the second instance connects and sends "activate"/"save-replay" then exits. Doubles as the CLI/hotkey control channel. |
| zbus service | 5.19.0 | active | ALT | Register `org.yourapp.Clips` on the session bus; `RequestName` failing = instance exists; `Activate` method -> xdg-activation token via `xdg_activation_v1` (eframe 0.37 honours `XDG_ACTIVATION_TOKEN` env on window creation). Cleanest for Wayland focus-stealing rules. |

## 14. Logging / config / dirs

| Crate | Version | Verdict | Notes |
|---|---|---|---|
| tracing / tracing-subscriber | 0.1.44 / 0.3.23 | USE | `tracing-appender` for rolling files; `tracing-journald` optional. |
| serde / serde_json | 1.0.229 / 1.0.154 | USE | |
| toml | 1.1.8 | USE | 1.x line (new parser); `toml_edit` if preserving comments. |
| directories | 6.0.0 | USE | `ProjectDirs::from("", "", "app")` -> XDG config/data/cache. |
| xdg | 3.0.0 | OK | Linux-only BaseDirectories; also handles `$XDG_RUNTIME_DIR` (needed for socket path). |
| etcetera | 0.11.0 | OK | Alternative with explicit XDG strategy. |

## 15. AppImage packaging

| Tool / crate | Version | Verdict | Notes |
|---|---|---|---|
| cargo-appimage | 2.4.0 (2025-11; repo 2025-08) | OK | Thin wrapper: builds, copies binary + `icon.png` + desktop file, optional `auto_link` lib bundling, calls `appimagetool`. Not a dependency resolver; fine for pure-Rust, weak for FFmpeg/GStreamer trees. |
| linuxdeploy (+ linuxdeploy-plugin-gstreamer) | continuous | USE | Standard; walks ELF deps, excludes the glibc/mesa/libva blacklist. Script it in CI; cargo-appimage adds little. |
| cargo-dist | 0.32.0 | OK | Produces tarballs/installers, not AppImages; can host the AppImage as an extra artifact. |
| appimage / appimage-rs | 0.4.0 / 0.2.0 | WATCH | Pure-Rust AppImage writer/reader; useful for a self-updater (`appimageupdate` 0.4.1 zsync). |

Guidance:
- glibc baseline: AppImage binaries link against the build host's glibc; build in a container of the oldest supported distro (Ubuntu 22.04 = glibc 2.35, or Debian 12 = 2.36). `cargo zigbuild --target x86_64-unknown-linux-gnu.2.35` works for the Rust side but not for bundled C libs -- so a container is simpler. Rust 1.9x needs glibc >= 2.17 only.
- Bundle FFmpeg as shared libs (libavcodec/format/util/filter/swscale/swresample + libx264 + libopus) built in the container, LGPL config (`--disable-gpl` unless you accept GPL for x264). Static FFmpeg into the Rust binary (`ffmpeg-sys-next` feature `static`/`build`) is possible but then VAAPI still dlopens the system `libva` + driver, so you cannot avoid the system dependency anyway.
- Never bundle libva, libdrm, Mesa, libgbm, libwayland*, libpipewire, libEGL/vulkan: use `wayland-client` with `dlopen`, `libva` with `dlopen`, and link libpipewire dynamically (it is on every modern desktop; a missing libpipewire should disable the portal path, not crash startup -- consider `libloading` for it).
- GStreamer-based pipeline + AppImage = bundle ~60 plugins + GLib + register `GST_PLUGIN_SYSTEM_PATH_1_0` in AppRun; linuxdeploy-plugin-gstreamer does it but expect breakage with `va` plugins depending on system Mesa. This is the main reason to prefer FFmpeg for an AppImage, or to ship Flatpak alongside (org.freedesktop.Platform has GStreamer + PipeWire + portals).

---

## Recommended stack (summary)

- Wayland: wayland-client 0.31 + wayland-protocols 0.32 (staging,client) + wayland-protocols-wlr 0.3 + sctk 0.21; no scanner.
- Capture tiers: ext-image-copy-capture (dmabuf) -> wlr-screencopy v3 (dmabuf) -> portal ScreenCast via ashpd + pipewire-rs (dmabuf or memfd) -> shm.
- Encode: ffmpeg-next 9.0 / ffmpeg-sys-next 9.0.1 with raw ffi for VAAPI hwframes; AAC native; in-memory GOP ring, normal mov muxing on save.
- Audio: pipewire-rs capture streams (sink monitor + mic) with SPA header timestamps.
- UI: egui/eframe 0.37 (wgpu 30, winit 0.30.13), egui_extras svg, egui-notify; overlay via a separate sctk layer-shell surface (not winit).
- IPC/secrets/upload: interprocess 2.4 (single instance + control), zbus 5.19 (+ashpd 0.13), keyring-core + zbus-secret-service store, russh 0.64 + russh-sftp 3.0, reqwest 0.13 (rustls).
- Packaging: linuxdeploy in an Ubuntu 22.04 container, bundle FFmpeg shared libs, dlopen Wayland/libva.
