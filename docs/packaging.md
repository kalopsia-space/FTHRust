# Packaging

FTHRust Clips is released as an AppImage built on Ubuntu 22.04, so it runs on any x86_64 Linux with glibc 2.35 or newer. This page covers what goes into it, how to build it, the gates it has to pass, and how a release is made.

- [What's in the AppImage](#whats-in-the-appimage)
- [Building it](#building-it)
- [The build, step by step](#the-build-step-by-step)
- [Gates](#gates)
- [Pinned inputs](#pinned-inputs)
- [Licences and notices](#licences-and-notices)
- [Releasing](#releasing)

## What's in the AppImage

```
AppRun                          execs usr/bin/fthrust with your arguments
space.kalopsia.fthrust.desktop  desktop entry (also in usr/share/applications)
space.kalopsia.fthrust.png      icon (also in usr/share/icons/hicolor/512x512)
LICENSE, THIRD_PARTY_NOTICES.md, licenses/
usr/bin/fthrust                 the app
usr/bin/fthrust-engine          the capture engine
usr/bin/fthrust-uploader        the uploader
usr/bin/assets/                 fonts, icons, sounds
usr/lib/libav*.so.*, libsw*.so.* a pinned LGPL FFmpeg 8.1
```

Binaries find the bundled FFmpeg through an `$ORIGIN/../lib` rpath. **PipeWire is deliberately not bundled.** It has to match the host's PipeWire daemon, so the AppImage loads the host's `libpipewire-0.3.so.0`. The same goes for Wayland, Vulkan, VA-API and GPU drivers.

## Building it

From any Linux machine with Docker or Podman:

```sh
packaging/docker-build.sh
```

This builds the `fthrust-appimage-build` image (Ubuntu 22.04 with the pinned Rust toolchain and cargo-about), then runs `packaging/build-appimage.sh` inside it as your user. The output is:

```
dist/FTHRust-<version>-x86_64.AppImage
dist/FTHRust-<version>-x86_64.AppImage.sha256
```

The container builds into `target/appimage/`, so it doesn't fight with your normal `target/`. Downloads are cached in `packaging/.cache/`. `dist/AppDir/` is left behind for inspection.

On an Ubuntu 22.04 host (like CI) you can run `packaging/build-appimage.sh` directly. Pass `--skip-build` to reuse existing release binaries.

`docker-build.sh` runs any command inside the build image, which is handy for tools your host lacks:

```sh
packaging/docker-build.sh tools/test-packaging.sh
```

## The build, step by step

`packaging/build-appimage.sh` does this, and nothing is written to `dist/` unless every step passes:

1. **Fetch pinned inputs** (`fetch-third-party.sh`): the FFmpeg build, `appimagetool`, the AppImage runtime, and PipeWire's source. Each download's size and SHA-256 is checked against its manifest.
2. **Build the PipeWire SDK** (`build-pipewire-sdk.sh`). Ubuntu 22.04 ships PipeWire 0.3.48, older than the Rust bindings need, so PipeWire 1.0.5 is built from source **for linking only**. None of it is shipped.
3. **Build** `fthrust`, `fthrust-engine` and `fthrust-uploader` with `--release --locked` against the pinned FFmpeg.
4. **Check notices**: `THIRD_PARTY_NOTICES.md` must match what cargo-about generates.
5. **Assemble the AppDir**: install the binaries with their rpath set by `patchelf`, then copy in the assets, FFmpeg libraries and soname links, `AppRun`, desktop file, icon and licences. Run `desktop-file-validate`.
6. **Gate the AppDir** with `verify-appdir.sh`.
7. **Pack** with `appimagetool` and the pinned runtime.
8. **Extract the packed image and gate it again**, so what's checked is what ships.
9. **Check versions**: every extracted binary must report the workspace version.
10. **Write** the AppImage and its `.sha256` into `dist/`.

## Gates

`packaging/verify-appdir.sh <AppDir>` prints `PASS` or `FAIL` for each of five checks and a final `RESULT:`:

1. **files**: every required file is present and the three binaries are executable.
2. **ffmpeg-libs**: the FFmpeg libraries are exactly the pinned ones, unmodified.
3. **forbidden-flags**: FFmpeg was built without `--enable-gpl`, `--enable-nonfree`, x264, x265, xvid or xavs2, so the bundle stays LGPL.
4. **linkage**: binaries use an `RPATH` (not `RUNPATH`) to `usr/lib`, link only the pinned FFmpeg sonames, and **don't bundle `libpipewire`**.
5. **glibc ceiling**: nothing needs a glibc newer than **2.35** (the baseline in `linux_release_baseline.json`).

`tools/test-packaging.sh` tests the gates themselves, by feeding them a good AppDir and deliberately broken ones (a tampered library, a `RUNPATH` instead of an `RPATH`, and so on). It needs `patchelf` and the fetched FFmpeg, so it's easiest to run through `docker-build.sh`.

## Pinned inputs

| Input | Pinned in | Notes |
|---|---|---|
| FFmpeg | `ffmpeg_manifest_linux.json` | BtbN's LGPL shared build of FFmpeg 8.1, with its sonames and forbidden configure flags. |
| appimagetool and runtime | `appimage_tool_manifest.json` | appimagetool 1.9.1 and the type-2 runtime. |
| PipeWire SDK | `pipewire_sdk_manifest.json` | PipeWire 1.0.5 source, for linking only. |
| Platform baseline | `linux_release_baseline.json` | x86_64, Ubuntu 22.04, glibc 2.35 at most. |
| Rust | `rust-toolchain.toml` | Also passed into the build image. |

To update an input, change its manifest (URL, size and SHA-256), rebuild, and let the gates tell you what broke.

## Licences and notices

`THIRD_PARTY_NOTICES.md` is generated by `tools/gen-notices.sh` with [cargo-about](https://github.com/EmbarkStudios/cargo-about). It starts with `packaging/notices/header.md` (FFmpeg, fonts and other non-Rust parts), then lists the licence of every crate linked into the shipped binaries. Dev and build dependencies are excluded. The accepted licences in `packaging/about.toml` mirror `deny.toml`.

After changing dependencies:

```sh
tools/gen-notices.sh           # regenerate
tools/gen-notices.sh --check   # what CI runs
```

## Releasing

1. Bump `version` in the root `Cargo.toml`. It's the only place the version lives.
2. Commit, then tag `v<version>` (for example `v2.0.0-alpha.2`) and push the tag.
3. `.github/workflows/release.yml` runs on Ubuntu 22.04. It checks the tag matches the workspace version, builds with `packaging/build-appimage.sh`, verifies the checksum, and uploads the AppImage and `.sha256` as the `fthrust-appimage-<commit>` artifact, kept for 7 days.
4. Download the artifact and attach both files to a GitHub release.

The workflow can also be started by hand (**Actions → release → Run workflow**) to build an AppImage from any branch without tagging.
