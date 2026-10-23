# Packaging and gating reference digest (plan 7 input)

Sources: `reference/` (Python app, gitignored in this repo: `git check-ignore reference` prints `reference`) and the Rust workspace at `/home/kalopsia/fthrrst-clips`. Line numbers are in the files as they stand on 2026-10-23. An earlier broader inventory exists at `docs/reference/reference-services-packaging-inventory.md` (section 3, "Linux packaging"); this note adds exact values and the Rust-side gap analysis.

## 1. How the reference packages and gates the Linux build

### 1.1 Build pipeline: `reference/build_linux.sh` (20354 bytes)

Order of steps:

1. Version read from `FTHR_UI/version.py` (`__version__ = '2.0.0-alpha'`, `VERSION_INFO = (2, 0, 0)`, `PRERELEASE = 'alpha'`, `BUILD_DATE = '2026-10-16'`, `APP_ID = 'FTHRClips'`). The script never hardcodes the version (comment "Never hardcode it here; tools/verify_version_consistency.py enforces that", around line 27-29).
2. Tool check: `cmake gcc pkg-config wayland-scanner curl readelf file patchelf sha256sum`.
3. `pkg-config --exists libpipewire-0.3 dbus-1` is headers only. Comment: "the engine dlopen()s these at runtime for the ScreenCast portal backend".
4. `python tools/fetch_third_party.py --ffmpeg-linux` fetches the pinned LGPL FFmpeg. Comment: "nothing here ever links /usr/lib FFmpeg".
5. Engine built with `-DFTHR_FFMPEG_ROOT=...`. Then `readelf -d` linkage check: every `lib(avcodec|avformat|avutil|avdevice|swscale|swresample).so.N` in DT_NEEDED must exist in the pinned tree, and the engine must have RPATH/RUNPATH, else "engine has no RPATH — it would load system libraries".
6. PyInstaller (`FTHR_linux.spec`), then strips unused libs, then removes non-pinned FFmpeg copies. The keep list is `_keep_regex='libavcodec\.so\.6[12]|libavformat\.so\.6[12]|libavutil\.so\.(59|60)|libavdevice\.so\.62|libavfilter\.so\.11|libswscale\.so\.[89]|libswresample\.so\.[56]'`. Any other `lib(av*|sw*|postproc)` is deleted, and a `strings | grep -- '--enable-gpl'` hit is logged as "GPL, removed".
7. `repair_stripped_elf.py`, then AppDir assembly: copy of the PyInstaller output, `AppDir/fthr-clips.desktop`, `AppDir/fthr-clips.png`, and a generated `AppRun`. `AppRun` sets `QT_QPA_PLATFORM` (`wayland;xcb` if `WAYLAND_DISPLAY`, else `xcb`) and `exec "$HERE/FTHRClips" "$@"`.
8. Fail-closed checks: no `desktop.ini`/`thumbs.db`/`ehthumbs.db` in AppDir; `FTHRClips`, `AppRun`, `.desktop` and `.png` must exist; `desktop-file-validate` runs if installed.
9. `cp LICENSE THIRD_PARTY_NOTICES.md licenses/` into AppDir, then `verify_release_licenses.py --appdir $APPDIR`.
10. `fetch_third_party.py --appimagetool-linux`, then `ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 appimagetool --runtime-file runtime-x86_64 AppDir out`. Packing happens in `$HOME/fthr-appimage-build` (a native filesystem; a WSL workaround).
11. glibc gate over the AppDir, the AppImage file, and the uploader outputs. On failure it deletes the outputs. Then it writes `FTHRClips-<ver>-x86_64.AppImage.sha256` (`sha256sum` run in the output dir).

Output name: `build_output/FTHRClips-${APP_VERSION}-x86_64.AppImage`.

### 1.2 AppDir contents (`reference/AppDir/`)

`fthr-clips.desktop`, the full file:

```
[Desktop Entry]
Type=Application
Name=FTHR Clips
Comment=Game capture and clip management
Exec=AppRun
Icon=fthr-clips
StartupWMClass=FTHR Clips
Categories=AudioVideo;Video;
Terminal=false
```

`fthr-clips.png` is 512x512, 8-bit colormap PNG. There is no hicolor tree, no appdata/metainfo and no `.DirIcon`. The only AppDir files are those two plus the generated `AppRun`.

### 1.3 Pinned tools: `reference/tools/appimage_tool_manifest.json` (33 lines)

- appimagetool 1.9.1, commit `8c8c91f762b412a19f4e8d2c4b35afb98f2d7c81`.
  - url: `https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage`
  - size 15092216
  - sha256 `ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0`
  - MIT, `build_only: true`
- type2-runtime tag `20251108`, commit `dd6cebedcbddde9c82f89b011e8e1d40b6e43868`.
  - url: `https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64`
  - size 944632
  - sha256 `2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d`
  - `license_file: licenses/AppImage-type2-runtime-LICENSE.txt`, `embedded_in_release: true`
- Fetcher: `fetch_third_party.py` `fetch_appimage_tools` (line 117), `_verified_appimage_asset` (line 85), `_verify_download` (line 69, size then sha256). It enforces a 64-hex sha256, https-only URL and an atomic replace.

### 1.4 FFmpeg manifest: `reference/tools/ffmpeg_manifest_linux.json`

- Format: JSON with `component`, `audit: "AUDIT-014"`, `version`, `license`, `platform`, `arch`, `retrieved`, `strategy{chosen,rejected,rationale,revisit_if}`, `source{project,release_tag,asset,url,sha256,size_bytes}`, `reproduce`, `toolchain`, `glibc{max_required_symbol_version,per_library,minimum_distribution}`, `forbidden_flags_absent`, `present_flags_of_note`, `software_encoders`, `hardware_encoders_verified_present`, `required_capabilities_verified`, `libraries_linked_by_engine`, `libraries_transitively_required`, `cli_tools`, `soname_map`, `shipped_files_sha256`, `additional_providers`, plus a note on why distro FFmpeg is forbidden.
- Version: `n8.1.2-34-g9b6c8969e0`, release_branch `8.1`, license "LGPL v3 or later", retrieved 2026-09-01.
- source:
  - project `BtbN/FFmpeg-Builds`, release_tag `autobuild-2026-07-31-14-10`
  - asset `ffmpeg-n8.1.2-34-g9b6c8969e0-linux64-lgpl-shared-8.1.tar.xz`
  - url `https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-07-31-14-10/ffmpeg-n8.1.2-34-g9b6c8969e0-linux64-lgpl-shared-8.1.tar.xz`
  - sha256 `c882a80f06617149198a98a07a0880a7e881953ae9f9cb931f5be09a4f93caae`
  - size_bytes 56666944
- glibc: `max_required_symbol_version` "2.28". Per library: `libavcodec.so.62`/`libavformat.so.62`/`libavutil.so.60`/`libavdevice.so.62`/`libavfilter.so.11` need `GLIBC_2.28`; `libswscale.so.9` needs `GLIBC_2.27`; `libswresample.so.6` needs `GLIBC_2.14`.
- Forbidden flags absent: `--enable-gpl --enable-nonfree --enable-libx264 --enable-libx265 --enable-libxvid --enable-libfdk-aac`. `--enable-version3` is allowed (LGPLv3).
- Present of note: `--enable-shared`, `--enable-libopenh264`, `--enable-libkvazaar`, `--enable-libsvtav1`, `--enable-ffnvcodec`, `--enable-vaapi`, `--enable-libvpl`, `--enable-libxcb`, `--enable-xlib`, `--enable-libpulse`.
- `libraries_linked_by_engine`: `libavcodec.so.62 libavformat.so.62 libavutil.so.60 libavdevice.so.62 libswscale.so.9 libswresample.so.6`. `libraries_transitively_required`: `libavfilter.so.11`.
- `soname_map` (versioned file to soname):
  - `libavcodec.so.62.28.102` -> `libavcodec.so.62`
  - `libavdevice.so.62.3.102` -> `libavdevice.so.62`
  - `libavfilter.so.11.14.102` -> `libavfilter.so.11`
  - `libavformat.so.62.12.102` -> `libavformat.so.62`
  - `libavutil.so.60.26.102` -> `libavutil.so.60`
  - `libswresample.so.6.3.102` -> `libswresample.so.6`
  - `libswscale.so.9.5.102` -> `libswscale.so.9`
- `shipped_files_sha256` (7 libs):
  - avcodec `e764cb8e423b837b7c600f23cee7188e548b1451ec190b4691b95386ea870fc3`
  - avdevice `e194f18571f675149dfa9d130df64e733b9af738008f3a34b4e3e807a9f8eee3`
  - avfilter `00b7baabe290369eab0f3ba2c31ae9c9ca21c2b46f62ebc32b3136238a186ea4`
  - avformat `fad88584a80c883b7c1d159b90b9e3b26a2df060de42099f709a75bb61dd7eda`
  - avutil `19e62b5ed04178807b0489d7cff8aa69d8f9bf99429a0740f9b438220ed5d248`
  - swresample `bb6e402d91995f38a80d650b0c166c44f0618f3611175eb3fca047d491dd337d`
  - swscale `88c41a7e04a4a686ffb695480dd885b59f91044b9ed268ca287f1adc21d33815`
- `cli_tools`: `ffmpeg`/`ffprobe` with rpath `$ORIGIN/../lib:$ORIGIN`, deliberately not hash-pinned because patchelf output varies. The Rust app has no CLI dependency, since exports run in-process (`CHANGES.md:13,56`).
- External codecs (openh264, kvazaar, svt-av1, dav1d, aom, vpx) are statically linked into the FFmpeg libs, so there is nothing extra to bundle.
- Fetch logic: `fetch_third_party.py` `fetch_ffmpeg_linux` (line 393). It downloads the archive and checks the archive sha256 ("archive sha256 mismatch", ~line 422), then per-file sha256 on the extracted libs (~line 453). It also fixes the CLI RPATH (line 245) and creates `.so` aliases (line 278). The headers come from the same archive: "Compile-time and bundled libraries must agree on the ABI."
- Windows sibling `tools/ffmpeg_manifest.json`: n8.1.2-21-gce3c09c101, tag `autobuild-2026-06-30-13-34`, sha256 `27bcaf58b5140171dfe838a0b365d12c60607d71fc168424456410bad6a834da`. Not relevant to the AppImage.

### 1.5 Gates

- **glibc: `reference/tools/verify_linux_glibc.py`** (240 lines) with `tools/linux_release_baseline.json`, exactly `{"architecture": "x86_64", "build_runner": "ubuntu-22.04", "max_glibc": "2.35"}`.
  - Walks `--target` (repeatable file or directory). It identifies ELF by the `\x7fELF` magic and runs `readelf --version-info --wide <file>`. It collects `\bGLIBC_(\d+(?:\.\d+)*)\b` and fails if any ELF's highest version exceeds `max_glibc`, if any target is missing, or if no ELF is found at all ("no ELF files found in the supplied targets"). Prints `RESULT: PASS|FAIL`; exit 0/1.
  - Weakness, already noted in the inventory doc: it does not extract an `.AppImage`. A target of the `.AppImage` file only sees the runtime's own ELF, so the real check is the pre-pack AppDir directory.
  - Two ceilings exist and they differ. FFmpeg manifest: 2.28 (what the BtbN libs need). Release baseline: 2.35 (the ubuntu-22.04 build runner, which sets the actual floor for the bundle).
- **Version consistency: `reference/tools/verify_version_consistency.py`** (164 lines), `main()` at line 113.
  - Source of truth is `FTHR_UI/version.py:__version__`.
  - Literal checks: `installer_windows.iss` (`#define MyAppVersion "..."`, plus a numeric form check) and `RELEASE_NOTES.md`. The regex is `(?m)^#+\s*(?:FTHR Clips\s+)?v?(\d+\.\d+\.\d+(?:-[A-Za-z0-9.]+)?)`; the first line is `# FTHR Clips 2.0.0-alpha`.
  - Derive-only checks: `FTHR_UI/main.py`, `build_linux.sh`, `FTHR.spec`, `FTHR_linux.spec` must contain no version literal. There is a small allow-list for unrelated versions.
  - CI tag guard (`ci.yml`, job `tag-guard`): `refs/tags/vX` must equal `version.__version__` exactly.
  - Gap in the reference: `package_source.py` hardcodes the version, and `RELEASE_NOTES.md` is only checked at its first heading.
- **License/provenance: `reference/tools/verify_release_licenses.py`** (1003 lines). Modes: `--tree`, `--windows-dist`, `--appdir`, `--all`.
  - `FORBIDDEN_FLAGS` (lines 22-29): `--enable-gpl`, `--enable-nonfree`, `--enable-libx264`, `--enable-libx265`, `--enable-libxvid`, `--enable-libxavs2`. These are matched as byte strings inside bundled binaries (`check_binary`, line 132).
  - `REQUIRED_ARTIFACT_FILES = ('LICENSE', 'THIRD_PARTY_NOTICES.md', 'licenses')` (line 85) must be at the artifact root.
  - `check_linux_manifest` (line 666): version/license/source/`shipped_files_sha256` present, LGPL in license, at least 7 library hashes.
  - `check_linux_ffmpeg_libs` (line 709): every FFmpeg-looking lib in the AppDir must be in the manifest's hash list, `soname_map` or `additional_providers`, with a license-banner check.
  - `check_linux_engine` (line 855): engine DT_NEEDED must not be a system FFmpeg soname, and RPATH must be `$ORIGIN`-relative.
- **Asset manifest hash gate: `reference/tools/release_asset_manifest.json`** (keys `schema_version, last_verified, allowed_redistribution_status="approved", scanned_source_roots[.github, AppDir, FTHR_UI/assets, installer_assets], asset_extensions, origins, assets[]`) plus `check_asset_manifest` (line 523).
  - Each entry has `path`, `sha256`, `license`, `copyright`, `origin` and `redistribution_status`, plus `packaging_references` and `artifact_paths`.
  - It fails on a hash mismatch, a missing asset, an undocumented asset found by scanning the roots (including `AppDir`, so the desktop icon must be listed), or an asset listed but outside the scan.
  - `check_asset_artifact` re-verifies hashes at the `artifact_paths.linux` locations inside the AppDir.
- **Qt manifest** `tools/qt_runtime_manifest.json` (PySide6 6.11.1 LGPL module allow-list). Not applicable to Rust.

### 1.6 CI: `reference/.github/workflows/ci.yml` (one file; also issue and PR templates)

- Triggers: push on `[main, master, linux, linux-2.0]`, tags `v*`, PRs. `permissions: contents: read`, no secrets, with a comment that none should ever be added.
- Jobs: `python`, `linux-engine`, `windows-engine`, `release-verification`, `tag-guard`, `linux-appimage-package`.
  - `release-verification` (ubuntu-latest, `fetch-depth: 0`) runs the licence, version-consistency and repo-hygiene gates plus a "No blobs above 2 MB in history" check.
  - `linux-appimage-package` (`runs-on: ubuntu-22.04`, needs `linux-engine` and `release-verification`) installs apt deps (`patchelf`, `libpipewire-0.3-dev`, ...), runs `bash build_linux.sh`, then `sha256sum --check`, then `verify_linux_glibc.py --manifest tools/linux_release_baseline.json --target build_output/AppDir --target <image> ...`. Artifact `fthr-linux-appimage-${{ github.sha }}`, retention 7 days.
  - `tag-guard` (`needs` includes the AppImage job) checks the tag against the version.
  - No release or publish job. The comment says "Publish artifacts manually after install, launch, and uninstall qualification."

### 1.7 Notices and licenses

- `reference/THIRD_PARTY_NOTICES.md` (21750 bytes) section layout:
  - "Licence of the distributed build" table (line 19)
  - "Distributed components" (47), with FFmpeg Windows (49) and "FFmpeg (Linux engine and AppImage)" (96), whose Source row (line 101) quotes the same tag/asset as the JSON
  - Qt, Python deps, "Wayland protocol definitions" (251)
  - "PipeWire and D-Bus headers (Linux engine)" (261)
  - "AppImage type-2 runtime" (279), "AppImage packaging tool (build-only)" (295)
  - "Project assets" (304), "Not distributed" (370), "Verifying this file" (388)
  - Line 382 states PipeWire/libdbus runtime libs are "loaded from the user's system with `dlopen()` ... never bundled".
- `reference/licenses/` (flat): `AppImage-type2-runtime-LICENSE.txt`, `FFmpeg-LICENSE.txt`, `MIT.txt`, `OpenH264-LICENSE.txt`, `NVIDIA-NVENC-SDK-LICENSE.txt`, `Oswald-OFL-1.1.txt`, `Wayland-Protocols-NOTICES.txt`, `FTHR-GENERATED-ASSETS.txt`, `FTHR-USER-PROVIDED-ASSETS.txt`, `KO-FI-BRAND-ASSETS.txt`, and Python and Qt items (cffi, jeepney, keyboard, numpy, opencv, pycparser, PySide6, Qt6, sounddevice) plus `optional-uploader/`. Rust-relevant ones: FFmpeg, OpenH264, NVENC, Oswald, Wayland, AppImage runtime, MIT.
- `reference/BUILDING.md` has no Linux section (it says "builds for Windows 10 and later"; Linux appears only in the uploader lock note). `RELEASE_NOTES.md` is per-version, with date sub-headings.

## 2. Rust repo today (`/home/kalopsia/fthrrst-clips`)

### Exists

| Item | Where | Details |
|---|---|---|
| Workspace | `Cargo.toml:1-9` | members `crates/*` (fthrust-core, -engine, -ipc, -media, -portal, -ui, -wayland); `[workspace.package] version = "2.0.0-alpha.1"`, `edition = "2024"`, `license = "GPL-3.0-only"`. Binaries: `fthrust` (`crates/fthrust-ui/Cargo.toml:44`) and `fthrust-engine` (`crates/fthrust-engine/Cargo.toml:30`). `tools/uidrive` is a separate workspace with its own `Cargo.lock`. |
| CI | `.github/workflows/ci.yml` (name `ci`, triggers `push:` and `pull_request:`) | Job `check` (ubuntu-24.04): fmt, `clippy -D warnings`, build engine, `cargo test --workspace`, `cargo install cargo-deny --locked`, `cargo deny check`. Job `uidrive` (ubuntu-latest): release build of ui and engine, plus the cage bench scenarios smoke, save_clip, export and overlays. Toolchain `dtolnay/rust-toolchain@1.98.1`. Both jobs install **distro** `libav*-dev` and `libpipewire-0.3-dev`. |
| cargo-deny | `deny.toml` (28 lines) | `[advisories] unmaintained = "all"` with ignores `RUSTSEC-2026-0370/0371/0372` (zbus 5.x sub-crates). `[licenses] allow` = GPL-3.0, GPL-3.0-only, MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0, MIT-0, BSL-1.0, bzip2-1.0.6, WTFPL, Apache-2.0 WITH LLVM-exception, OFL-1.1, Ubuntu-font-1.0, CC0-1.0. No `[bans]`, `[sources]` or `[graph]` sections. |
| Asset hash gate | `assets/manifest.sha256` (54 lines, `sha256sum` format, paths `./favicon.ico`...) and `tools/check-assets.sh` | Checks every asset against the reference copy and runs `sha256sum -c`. It silently skips the reference comparison if `reference/` is absent. Not run in CI. No per-asset license/origin metadata. |
| Assets | `assets/` | `favicon.ico`, `fonts/Oswald-{Regular,SemiBold,Bold}.ttf`, `fthr_logo.png`, `gary.png`, `gatze.png`, `preview_desktop.png`, `icons/*`, `sounds/*.wav`. |
| App id | `crates/fthrust-ui/src/main.rs:86` | `.with_app_id("fthrust")`. |
| Version display | `README.md:20` | `fthrust --version` prints the version. |
| Docs | `docs/reference/reference-services-packaging-inventory.md`, `CHANGES.md`, `LICENSE` | |

### Missing (confirmed absent)

Checked with `git ls-files | grep -iE 'desktop|packag|appimage|build\.rs'` and `ls packaging` (no such dir); there is no `crates/*/build.rs`.

- **AppImage build script**: none. There is no `packaging/` dir, no `AppRun`, no appimagetool or runtime manifest, no fetcher.
- **Desktop file**: none. The id `space.kalopsia.fthrust` appears nowhere in the repo, and the code uses `with_app_id("fthrust")`. For Wayland the `.desktop` basename must equal the app id, so either the window app id changes to `space.kalopsia.fthrust` (`main.rs:86`) or the desktop file is named `fthrust.desktop`. The reference used `StartupWMClass=FTHR Clips`, which has no Wayland equivalent.
- **Icon**: the repo has no icon sized or named for a desktop entry (`assets/fthr_logo.png`, `favicon.ico`). The reference's 512x512 colormap PNG is at `reference/AppDir/fthr-clips.png`. The AppImage needs `space.kalopsia.fthrust.png` (root and `usr/share/icons/hicolor/512x512/apps/`) and `.DirIcon`. A new icon must also go into the asset hash gate, because the reference gate scans `AppDir` as an asset root.
- **THIRD_PARTY_NOTICES**: none, and no `licenses/` dir. There is no cargo-about or cargo-license configuration. `deny.toml` only allow-lists SPDX ids; it does not generate text. There is no FFmpeg, OpenH264, NVENC header, Wayland protocol, AppImage runtime or Oswald OFL notice yet.
- **FFmpeg manifest**: none (no `ffmpeg_manifest*.json`, no fetcher, no sha256 pins). No gate forbids GPL FFmpeg flags.
- **glibc gate**: none.
- **Version consistency gate**: none. Version lives in `Cargo.toml` (`2.0.0-alpha.1`) and `CHANGES.md` (70 lines, no version heading convention checked). The reference version is `2.0.0-alpha`. Decide whether the Rust app keeps `-alpha.1`.
- **Release workflow / tag guard**: none. CI triggers on every push and has no `permissions:` block and no tags filter. Nothing builds a release-profile AppImage artifact. There is no `[profile.release]` in `Cargo.toml` (lto/strip/panic unset).
- No `target/release` exists locally.

## 3. Runtime linkage of the Rust binaries (`target/debug`, built 2026-10-23, build host glibc 2.44)

ffmpeg-next 9 comes from `Cargo.toml:15` (`ffmpeg-next = "9"`) and `Cargo.lock` (`ffmpeg-sys-next 9.0.1`). It has no vendored or static features set and no `.cargo/config.toml` or `build.rs`, so the default is pkg-config discovery of system shared FFmpeg. Only `fthrust-engine` and `fthrust-media` depend on it (`crates/fthrust-engine/Cargo.toml:11`, `crates/fthrust-media/Cargo.toml:11`), but the `fthrust` UI binary links it through fthrust-media.

`readelf -d` NEEDED:

- `target/debug/fthrust`: `libpipewire-0.3.so.0 libavutil.so.61 libavformat.so.63 libavfilter.so.12 libavdevice.so.63 libswscale.so.10 libavcodec.so.63 libgcc_s.so.1 libm.so.6 libc.so.6 ld-linux-x86-64.so.2`. Max `GLIBC_2.44`. It has no `libswresample` NEEDED.
- `target/debug/fthrust-engine`: `libpipewire-0.3.so.0 libavutil.so.61 libavformat.so.63 libavdevice.so.63 libswscale.so.10 libswresample.so.7 libavcodec.so.63 libgcc_s.so.1 libm.so.6 libc.so.6 ld-linux-x86-64.so.2`. Max `GLIBC_2.39`. It has no `libavfilter` NEEDED.
- Neither binary has RPATH or RUNPATH.
- `ldd` expands to about 180 libs, because the Arch system FFmpeg 9.x (`pkg-config --modversion libavcodec` = 63.1.102) pulls in libx264.so.165, libx265.so.217, libxvidcore.so.4 and many others. This is exactly the GPL-contamination failure that reference AUDIT-014 describes.
- PipeWire: linked directly through the `pipewire` crate (0.10, feature `v0_3_49`; used by -ui, -portal, -engine). It is a hard DT_NEEDED on `libpipewire-0.3.so.0` (host 1.6.10). The reference instead dlopen()ed PipeWire/libdbus and never bundled them. The Rust app cannot start on a host without libpipewire unless the library is bundled or loading is changed. Bundling libpipewire is a poor idea, because it must match the host's daemon and SPA plugins. The usual choice is to exclude it from the AppImage and require the host's libpipewire-0.3.so.0.
- Wayland: `wayland-client` has `dlopen`, and `wayland-backend` has `client_system` + `dlopen` (`Cargo.toml:29-30`), so there is no libwayland DT_NEEDED. winit uses wayland with `dlopen`/xkbcommon loaded dynamically; `ldd` shows no libwayland, libxkbcommon or libvulkan, but `libvulkan` and `libX11`/`libxcb` appear transitively via ffmpeg. wgpu loads Vulkan/GL drivers at runtime (host drivers, not bundled).
- D-Bus: the portal path is pure Rust (zbus/ashpd), with no libdbus. `libdbus-1.so.3` in the ldd output comes from system ffmpeg's transitive deps.

### Implications for the AppImage

1. **ABI mismatch to resolve first.** The BtbN pin used by the reference is FFmpeg 8.1 (`.so.62/.60/.11/.9/.6`). The Rust workspace is currently built against the host's FFmpeg 9 (`.so.63/.61/.12/.10/.7`). `ffmpeg-sys-next 9.0.1` `build.rs` (~line 1000-1013) has feature steps up to `("ffmpeg_8_1", 62, 28)` and `("ffmpeg_9_0", 63, 1)`, so it should accept the 8.1 headers. That is not verified here. CI also builds against Ubuntu 24.04's distro FFmpeg 6.1 (GPL build). Plan 7 must point the build at the extracted BtbN tree (for example `PKG_CONFIG_PATH=<tree>/lib/pkgconfig` or `FFMPEG_DIR`) in both CI and the release build. Otherwise the AppImage will be built against, and linked to, a GPL FFmpeg.
2. Needed FFmpeg set: the UI binary needs `libavfilter` too (so 7 libs total, as in the reference manifest). The engine needs swresample, but the UI does not.
3. Set RPATH to `$ORIGIN/../lib` (patchelf or `-C link-arg=-Wl,-rpath,'$ORIGIN/../lib'`) and run the reference's `check_linux_engine` equivalent. Today there is no RPATH, so the loader would silently use host FFmpeg libs.
4. glibc floor: BtbN libs need at most 2.28. The binaries themselves currently need up to 2.39 or 2.44 because they were built on Arch. The reference built on ubuntu-22.04 and gated at 2.35. Rust CI uses ubuntu-24.04 (glibc 2.39), so a release job should use ubuntu-22.04 (or a container) and a baseline JSON equivalent to the reference's. Also check that libstdc++ is not needed: current NEEDED shows no `libstdc++`, so the bindgen/cc build does not pull it in.
5. Exclude from the AppImage (host-provided): libc/libm/libgcc_s, libpipewire-0.3, GPU drivers and Vulkan/GL, libwayland and libxkbcommon (dlopen'd). Bundle: the 7 pinned FFmpeg libs from the BtbN archive, plus `LICENSE`, `THIRD_PARTY_NOTICES.md`, `licenses/`, and the `assets/` the app locates at runtime. Check the asset lookup path logic for AppDir use, since the reference put assets in `_internal/assets`.
6. Both binaries are needed in the AppDir (`fthrust` UI plus `fthrust-engine`), and the app re-invokes itself for the encoder probe (`--fthrust-probe-encoder`, `CHANGES.md:56`).

## 4. Gap checklist for plan 7

1. `packaging/ffmpeg_manifest_linux.json` copied from the reference (same URL and hashes above), a Rust or shell fetcher with archive and per-lib sha256 checks, and a rule for pointing ffmpeg-sys-next at the tree.
2. `packaging/appimage_tool_manifest.json` (same pins), with a fetch and verify step for appimagetool and runtime.
3. `packaging/space.kalopsia.fthrust.desktop` (suggested `Exec=fthrust`, `Icon=space.kalopsia.fthrust`, `Categories=AudioVideo;Video;`, `Terminal=false`, `StartupWMClass=space.kalopsia.fthrust`), the icon, optional metainfo, and `main.rs:86` changed to the new app id.
4. `packaging/build-appimage.sh` (cargo build --release, assemble AppDir with `AppRun`, `usr/bin`, `usr/lib`, the pinned FFmpeg libs, patchelf RPATH, `desktop-file-validate`, pack, sha256 file).
5. Notices: generate `THIRD_PARTY_NOTICES.md` from `cargo about` or `cargo-license` output, plus hand-written FFmpeg, OpenH264 and Wayland sections mirroring reference lines 96-115 and 251-269. Copy `licenses/` files from the reference. Add `[bans]` and `[sources]` to `deny.toml` and keep `licenses.allow` in sync with what the notices generator accepts.
6. A gate script (`tools/verify-release.sh` or a Rust xtask) that does the following.
   - Run the glibc check on the **extracted/pre-pack AppDir** and on `appimage-extract` output (not on the raw image), with a baseline JSON `{"architecture": "x86_64", "build_runner": "ubuntu-22.04", "max_glibc": "2.35"}`.
   - Do the FFmpeg flag, DT_NEEDED and RPATH checks (port `FORBIDDEN_FLAGS`, `check_linux_ffmpeg_libs`, `check_linux_engine`).
   - Check that required notice files are present at the AppDir root.
7. Version gate: `Cargo.toml` `[workspace.package].version`, the first `CHANGES.md` heading, the AppImage filename, the tag `vX` (strip the leading `v`, and compare to the exact Cargo version), and optionally `fthrust --version` output from the built binary.
8. `.github/workflows/release.yml` (or an extra job in `ci.yml`) on `tags: ['v*']`, `runs-on: ubuntu-22.04`, with `permissions: contents: read` for verification jobs and `contents: write` only in the publishing step. The reference deliberately has no publish step, so any automatic publishing is new.
9. Add `tools/check-assets.sh` (or its successor) to CI, make it hash the new icon, and add the `reference/`-absent behavior as a hard failure in release mode.
