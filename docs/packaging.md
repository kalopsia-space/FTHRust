# Packaging

FTHRust ships as one AppImage, `FTHRust-<version>-x86_64.AppImage`, with a
`.sha256` next to it. The version is `[workspace.package].version` in
`Cargo.toml`; no script hardcodes it.

## Building

```sh
packaging/docker-build.sh                 # any host with Docker
packaging/build-appimage.sh               # on Ubuntu 22.04 itself (CI)
```

`docker-build.sh` builds the Ubuntu 22.04 image from `packaging/Dockerfile`
(toolchain from `rust-toolchain.toml`, cargo-about from `tools/gen-notices.sh`)
and runs any command inside it with the repository at `/src`; with no command it
runs `build-appimage.sh`. Release binaries go to `target/appimage/`, the
registry cache to `target/appimage-registry/`, and the outputs to `dist/`
(`dist/AppDir` stays for inspection).

The build:

1. fetches the pinned inputs into `packaging/.cache/` with
   `packaging/fetch-third-party.sh` (https only, size then sha256, atomic
   replace; a tampered cache entry is downloaded again);
2. builds the PipeWire 1.0.5 headers and link library
   (`packaging/build-pipewire-sdk.sh`) because Ubuntu 22.04's PipeWire 0.3.48
   is older than `libspa` 0.10 needs. The SDK is build-only;
3. builds `fthrust`, `fthrust-engine` and `fthrust-uploader` with
   `PKG_CONFIG_PATH` pointing at the BtbN LGPL FFmpeg 8.1 tree and the SDK;
4. assembles the AppDir: binaries and `assets/` in `usr/bin/`, the seven
   FFmpeg libraries (unmodified) and their soname links in `usr/lib/`, the
   desktop file and icon, `LICENSE`, `THIRD_PARTY_NOTICES.md`, `licenses/`.
   `patchelf --force-rpath` gives each binary `DT_RPATH=$ORIGIN/../lib`.
   `DT_RPATH`, unlike `DT_RUNPATH`, also applies to the libraries' own
   dependencies (libavcodec → libswresample) and outranks `LD_LIBRARY_PATH`;
5. gates the AppDir, packs it with the pinned appimagetool and type-2 runtime,
   extracts the result and gates it again, checks with `LD_DEBUG=libs` that
   both binaries load FFmpeg only from the image, and checks `--version`;
6. writes the AppImage and `.sha256` to `dist/`. Nothing reaches `dist/`
   unless every gate passed.

## Gates

| Script | Checks |
|---|---|
| `packaging/verify-appdir.sh <dir>` | required files; every bundled FFmpeg file is pinned in `ffmpeg_manifest_linux.json` with a matching hash and soname link; no ELF contains a GPL/nonfree configure flag; each binary has exactly `DT_RPATH=$ORIGIN/../lib`, no `DT_RUNPATH`, only pinned FFmpeg sonames; no bundled libpipewire; highest `GLIBC_` symbol ≤ `linux_release_baseline.json` (2.35). A file argument is refused, because the gate must see the extracted tree. |
| `tools/check-version.sh [--tag REF] [--binary PATH]` | every crate inherits the workspace version; a tag is exactly `v<version>`; binaries print it. |
| `tools/gen-notices.sh --check` | `THIRD_PARTY_NOTICES.md` matches `packaging/notices/header.md` plus cargo-about's crate licences. |
| `tools/check-assets.sh` | `assets/` and `packaging/` hash manifests; byte-identity with the reference when `reference/` is present. |
| `tools/test-packaging.sh` | tests for the fetcher, the gate and the version check (the gate group needs `patchelf`; run it through `docker-build.sh` on hosts without it). |

## Host runtime dependencies

Not bundled: glibc ≥ 2.35, `libpipewire-0.3.so.0` (PipeWire ≥ 0.3.49), libva
and the VA-API driver, NVIDIA drivers for NVENC, Vulkan/GL drivers, libwayland,
libxkbcommon, and an xdg-desktop-portal backend with ScreenCast, Screenshot and
GlobalShortcuts.

## App id

The Wayland app id, desktop file and icon name are all `space.kalopsia.fthrust`.
The design spec section 7 named the desktop file `fthrust.desktop`; it changed
because the portal registration (`services::APP_ID`) already uses
`space.kalopsia.fthrust`, and desktop environments match the desktop file to the
window and portal session by that basename.

## Bumping a pin

Edit the manifest in `packaging/`, run `packaging/fetch-third-party.sh` (it
refuses a mismatch), rebuild, and update the matching section of
`packaging/notices/header.md`, then `tools/gen-notices.sh`. After a dependency
change, `tools/gen-notices.sh` regenerates `THIRD_PARTY_NOTICES.md`.

## Releases

`.github/workflows/release.yml` runs on `v*` tags and on manual dispatch,
builds on `ubuntu-22.04`, and uploads the AppImage and `.sha256` as a workflow
artifact. It does not publish; publish manually after testing the build on a
desktop.
