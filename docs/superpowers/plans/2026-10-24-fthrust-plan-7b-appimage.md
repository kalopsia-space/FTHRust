# FTHRust Plan 7b: AppImage Packaging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `dist/FTHRust-<version>-x86_64.AppImage` (plus `.sha256`) from the workspace against the pinned LGPL BtbN FFmpeg 8.1 on a glibc 2.35 baseline, with release gates that inspect the AppDir and the extracted image, and CI that runs them.

**Architecture:** `packaging/` holds pinned manifests (FFmpeg, appimagetool and runtime), a fetcher that checks size and sha256, the desktop file and icon, an Ubuntu 22.04 Dockerfile, and `build-appimage.sh`, which runs natively on Ubuntu 22.04 (CI) or inside the container (`packaging/docker-build.sh`, for local builds on Arch). The binaries get `DT_RPATH=$ORIGIN/../lib` so the seven pinned FFmpeg libraries ship byte-identical and still resolve each other. `packaging/verify-appdir.sh` is the release gate (required files, FFmpeg hashes and flags, linkage, glibc ceiling), and it runs on the AppDir before packing and on the `--appimage-extract` output after.

**Tech Stack:** bash, jq, patchelf, meson/ninja (PipeWire link SDK), appimagetool 1.9.1 + type2-runtime 20251108, cargo-about, Docker (`ubuntu:22.04`), GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md` sections 4 and 6.6 (and 7 for the desktop id). Reference digest with every pin: `docs/superpowers/notes-packaging-reference.md` (section numbers below are its sections).

## Global Constraints

- Glibc ceiling 2.35 (`packaging/linux_release_baseline.json` = `{"architecture": "x86_64", "build_runner": "ubuntu-22.04", "max_glibc": "2.35"}`); release binaries are built on Ubuntu 22.04.
- FFmpeg: exactly the BtbN pin in digest 1.4 (url, archive sha256 `c882a80f…caae`, size 56666944, the seven `shipped_files_sha256`, the `soname_map`). Nothing links or bundles a distro FFmpeg. Forbidden strings in any bundled ELF: `--enable-gpl`, `--enable-nonfree`, `--enable-libx264`, `--enable-libx265`, `--enable-libxvid`, `--enable-libxavs2`.
- PipeWire link SDK (build only, never shipped): Ubuntu 22.04's PipeWire 0.3.48 headers are too old for `libspa` 0.10 (probe on 2026-10-24: `spa_meta_region_is_valid` missing, `spa_video_info_raw.flags` missing), so the build compiles headers and `libpipewire-0.3.so` from `https://gitlab.freedesktop.org/pipewire/pipewire/-/archive/1.0.5/pipewire-1.0.5.tar.gz` (size 2278062, sha256 `c5a5de26d684a1a84060ad7b6131654fb2835e03fccad85059be92f8e3ffe993`) with `meson setup build --prefix=<sdk> --libdir=lib -Dauto_features=disabled -Dflatpak=disabled -Ddbus=disabled -Dsession-managers=[] -Dexamples=disabled -Dtests=disabled`. The binaries keep `libpipewire-0.3.so.0` as a host dependency.
- Probe result to preserve: with `PKG_CONFIG_PATH=<ffmpeg>/lib/pkgconfig:<pipewire-sdk>/lib/pkgconfig` on Ubuntu 22.04, `cargo build --release --locked -p fthrust-ui -p fthrust-engine -p fthrust-uploader` succeeds against FFmpeg 8.1; max glibc is `GLIBC_2.35` (`fthrust`), `2.34` (engine, uploader); NEEDED FFmpeg sonames `.so.62/.60/.11/.9/.6`.
- appimagetool and runtime: digest 1.3 pins (url, size, sha256). Downloads are https only, checked for size and then sha256, and replaced atomically.
- Never bundled (host-provided per spec 4): libc/libm/libgcc_s/libpthread/libdl/librt/libmvec, `libpipewire-0.3.so.0`, libva and GPU drivers, libwayland, libxkbcommon, Vulkan/GL.
- App id: `space.kalopsia.fthrust` everywhere — Wayland window app id, desktop file `space.kalopsia.fthrust.desktop`, icon name. This replaces the spec 7 `fthrust.desktop`, because `services::APP_ID` already registers `space.kalopsia.fthrust` with the portal and the desktop file basename must match it (a `CHANGES.md`-style note goes in `docs/packaging.md`).
- Desktop entry keys: `Type=Application`, `Name=FTHR Clips`, `Comment=Game capture and clip management`, `Exec=fthrust`, `Icon=space.kalopsia.fthrust`, `Categories=AudioVideo;Video;`, `Terminal=false`, `StartupWMClass=space.kalopsia.fthrust`. The icon is the reference `AppDir/fthr-clips.png` unchanged (512×512).
- Output names: `dist/FTHRust-${VERSION}-x86_64.AppImage` and `dist/FTHRust-${VERSION}-x86_64.AppImage.sha256` (written by `sha256sum` run inside `dist/`). `VERSION` comes from `cargo metadata` (workspace version, currently `2.0.0-alpha.1`); no script hardcodes it.
- CI never rewrites a tracked file (spec 9 packaging defect); generators have a `--check` mode used by CI.
- No publish step; release CI uploads the AppImage as a workflow artifact only (`permissions: contents: read`).

## Review Focus

1. Running the AppImage on a host whose system FFmpeg has the same sonames (Ubuntu 22.04 FFmpeg is `.so.58`, but a host could have `.so.62` with GPL x264): the bundled libraries win. The `verify-appdir.sh` linkage check pins `DT_RPATH` exactly and rejects `DT_RUNPATH`, and Task 5 runs the extracted binary with `LD_DEBUG=libs` and asserts every `libav*`/`libsw*` resolves under the AppDir.
2. The libraries' own dependencies (avcodec → swresample, avdevice → avfilter): they resolve from the AppDir even though the UI binary has no direct `libswresample` NEEDED. Covered by the same `LD_DEBUG` check in Task 5, run on both `fthrust` and `fthrust-engine`.
3. Assets, engine and uploader found inside the mounted image (read-only `/tmp/.mount_*`): `assets_dir()` must prefer `<exe dir>/assets` over the compile-time repository path, or a build-host path baked in by `env!("CARGO_MANIFEST_DIR")` wins on the builder's own machine. Test in Task 1.
4. A tampered or truncated download in the cache: the fetcher refuses it and re-downloads; a manifest edited to a non-https URL or a malformed sha256 is refused before any network access. Tests in Task 2.
5. A gate fed the wrong thing: `verify-appdir.sh` given a path with no ELF files, or a raw `.AppImage` file instead of a directory, fails loudly instead of passing. Test in Task 4.

---

## File structure

```
packaging/
  ffmpeg_manifest_linux.json        BtbN pin (digest 1.4 fields that the gate reads)
  appimage_tool_manifest.json       appimagetool + runtime pins (digest 1.3)
  pipewire_sdk_manifest.json        PipeWire 1.0.5 source pin (build only)
  build-pipewire-sdk.sh             meson build of the link SDK into packaging/.cache/pipewire-sdk
  linux_release_baseline.json       glibc ceiling
  fetch-third-party.sh              fetch + verify into packaging/.cache/ (gitignored)
  space.kalopsia.fthrust.desktop
  space.kalopsia.fthrust.png        reference AppDir/fthr-clips.png, unchanged
  AppRun
  Dockerfile                        ubuntu:22.04 build image
  docker-build.sh                   local wrapper: build image, run build-appimage.sh inside
  build-appimage.sh                 release build, AppDir, gates, pack, sha256
  verify-appdir.sh                  release gate
  notices/header.md                 hand-written non-crate sections of THIRD_PARTY_NOTICES.md
  about.toml, about.hbs             cargo-about config and template
  manifest.sha256                   hashes of packaging assets (icon)
licenses/                           FFmpeg, OpenH264, NVENC SDK, Oswald OFL, Wayland protocols, AppImage runtime, MIT
THIRD_PARTY_NOTICES.md              generated, committed, checked fresh in CI
tools/gen-notices.sh                writes THIRD_PARTY_NOTICES.md; --check compares
tools/check-version.sh              version consistency (+ tag guard)
tools/test-packaging.sh             shell tests for the fetcher and the gate
.github/workflows/release.yml       AppImage job
docs/packaging.md
```

---

### Task 1: App id, asset lookup, desktop entry and icon

**Files:**
- Modify: `crates/fthrust-ui/src/main.rs:86`, `crates/fthrust-ui/src/lib.rs:147-161`
- Create: `packaging/space.kalopsia.fthrust.desktop`, `packaging/space.kalopsia.fthrust.png`, `packaging/manifest.sha256`
- Modify: `tools/check-assets.sh`
- Test: `crates/fthrust-ui/tests/assets_dir.rs`

**Interfaces:**
- Produces: `pub fn assets_dir_for(exe: Option<&Path>, dev: &Path) -> PathBuf` in `fthrust-ui/src/lib.rs` (`assets_dir()` calls it after the `FTHRUST_ASSETS` override); the window app id is `services::APP_ID`.

- [ ] **Step 1: Write the failing tests** — `exe_adjacent_assets_win_over_dev_tree` (tempdir with `bin/fthrust` and `bin/assets/`, plus a dev dir that exists → returns `bin/assets`); `dev_tree_used_when_no_adjacent_assets` (no `bin/assets` → returns canonical dev dir); `falls_back_to_relative_assets` (neither exists, `exe = None` → `PathBuf::from("assets")`).
- [ ] **Step 2: Run** `cargo test -p fthrust-ui --test assets_dir` — Expected: FAIL, `assets_dir_for` not found.
- [ ] **Step 3: Implement** `assets_dir_for` with the order exe-adjacent → dev tree → `"assets"`; change `.with_app_id("fthrust")` to `.with_app_id(services::APP_ID)`.
- [ ] **Step 4: Run** the test again — Expected: PASS.
- [ ] **Step 5: Add the desktop file** with the Global Constraints keys, copy `reference/AppDir/fthr-clips.png` to `packaging/space.kalopsia.fthrust.png`, and write `packaging/manifest.sha256` (`sha256sum` format, path `./space.kalopsia.fthrust.png`). In `tools/check-assets.sh`, always run the `sha256sum -c` checks (assets and packaging), and skip only the reference comparison when `reference/` is absent; when present, also compare the icon with `reference/AppDir/fthr-clips.png`.
- [ ] **Step 6: Verify** `desktop-file-validate packaging/space.kalopsia.fthrust.desktop` prints nothing, and `tools/check-assets.sh` exits 0 with and without `reference/` (`mv reference /tmp/x` round trip is fine).
- [ ] **Step 7: Commit** `feat(packaging): app id space.kalopsia.fthrust, desktop entry, icon, exe-adjacent assets`.

### Task 2: Pinned manifests and the fetcher

**Files:**
- Create: `packaging/ffmpeg_manifest_linux.json`, `packaging/appimage_tool_manifest.json`, `packaging/pipewire_sdk_manifest.json`, `packaging/linux_release_baseline.json`, `packaging/fetch-third-party.sh`, `packaging/build-pipewire-sdk.sh`, `tools/test-packaging.sh`
- Modify: `.gitignore` (add `/packaging/.cache`, `/dist`)

**Interfaces:**
- Produces: `packaging/fetch-third-party.sh [--ffmpeg] [--appimagetool] [--pipewire] [--cache DIR] [--manifest-dir DIR]` (default all, cache `packaging/.cache`). After success it prints the paths and leaves `$CACHE/ffmpeg/` (the extracted tree with `lib/`, `include/`, `lib/pkgconfig/`), `$CACHE/appimagetool-x86_64.AppImage` (mode 755), `$CACHE/runtime-x86_64` and `$CACHE/pipewire-1.0.5.tar.gz`. `packaging/build-pipewire-sdk.sh [--cache DIR]` builds `$CACHE/pipewire-sdk/lib/pkgconfig/{libpipewire-0.3,libspa-0.2}.pc` from the tarball (skipped when `$CACHE/pipewire-sdk/.version` already says `1.0.5`). Exit 1 with `fetch: <reason>` on any failure.
- Manifest JSON shape the gate also reads: ffmpeg `{version, license, source{url, sha256, size_bytes, asset}, soname_map{file: soname}, shipped_files_sha256{file: sha256}, forbidden_flags[]}`; tools `{appimagetool{version, url, size, sha256, license}, runtime{tag, url, size, sha256, license_file}}`. Values are copied from the digest 1.3/1.4.

- [ ] **Step 1: Write the failing tests** in `tools/test-packaging.sh` (bash, each case a function, prints `ok <name>` or `FAIL <name>` and exits non-zero on any failure; uses a temp cache and a temp copy of the manifests): `rejects_http_url` (url rewritten to `http://` → exit 1, message contains `https`, and no file appears in the cache); `rejects_bad_sha_format` (sha256 `abc` → exit 1 before download); `refetches_tampered_cache` (fetch `--appimagetool`, append a byte to the cached runtime, fetch again → exit 0 and the runtime's sha256 matches the pin again); `rejects_size_mismatch` (manifest size off by one → exit 1, nothing left in the cache).
- [ ] **Step 2: Run** `tools/test-packaging.sh fetch` — Expected: FAIL (script missing).
- [ ] **Step 3: Implement** `fetch-third-party.sh`: validate the manifest (https, 64-hex sha256) first; a cached file with the right size and hash is reused; otherwise `curl -fL` to `<dest>.part`, check size then sha256, `mv` into place. FFmpeg: verify the archive, extract into `$CACHE/ffmpeg.tmp`, move the inner `ffmpeg-*` dir to `$CACHE/ffmpeg`, then check every `shipped_files_sha256` entry under `lib/` (mismatch → delete the tree, exit 1) and that `bin/` is removed (the app has no CLI dependency).
- [ ] **Step 4: Run** `tools/test-packaging.sh fetch` — Expected: all `ok`. Then `packaging/fetch-third-party.sh && packaging/build-pipewire-sdk.sh` — Expected: exit 0 and `PKG_CONFIG_PATH=packaging/.cache/pipewire-sdk/lib/pkgconfig pkg-config --modversion libpipewire-0.3` prints `1.0.5`.
- [ ] **Step 5: Commit** `feat(packaging): pinned FFmpeg and AppImage tool manifests with verified fetcher`.

### Task 3: Third-party notices and licences

**Files:**
- Create: `licenses/*` (copied unchanged from `reference/licenses/`: `AppImage-type2-runtime-LICENSE.txt`, `FFmpeg-LICENSE.txt`, `MIT.txt`, `OpenH264-LICENSE.txt`, `NVIDIA-NVENC-SDK-LICENSE.txt`, `Oswald-OFL-1.1.txt`, `Wayland-Protocols-NOTICES.txt`), `packaging/notices/header.md`, `packaging/about.toml`, `packaging/about.hbs`, `tools/gen-notices.sh`, `THIRD_PARTY_NOTICES.md`
- Modify: `deny.toml` (add `[bans] multiple-versions = "allow"` with `wildcards = "deny"` and `[sources] unknown-registry = "deny"`, `unknown-git = "deny"`)

**Interfaces:**
- Produces: `tools/gen-notices.sh [--check] [--out FILE]` (default out `THIRD_PARTY_NOTICES.md`); `--check` exits 1 with `notices out of date: run tools/gen-notices.sh` when the regenerated text differs. It needs `cargo-about` on PATH and pins its version in a `CARGO_ABOUT_VERSION=` line at the top (the current release on crates.io at implementation time).

- [ ] **Step 1: Write `header.md`** from reference `THIRD_PARTY_NOTICES.md` sections "Licence of the distributed build", FFmpeg (Linux) with the source row quoting the manifest's tag, asset and sha256, "Wayland protocol definitions", "AppImage type-2 runtime", "AppImage packaging tool (build-only)", "Project assets" (Oswald, icons, sounds), and "Not distributed" rewritten for this app: PipeWire, libva and GPU drivers come from the host and are never bundled (no dlopen claim: `fthrust` links `libpipewire-0.3.so.0` directly). Drop the Qt, Python and Windows sections.
- [ ] **Step 2: Write `about.toml`** (`accepted` = the `deny.toml` `[licenses] allow` list, `targets = ["x86_64-unknown-linux-gnu"]`, `ignore-dev-dependencies = true`, `ignore-build-dependencies = true`) and `about.hbs` (one `### <name> <version>` per crate grouped under each licence, then each licence text once).
- [ ] **Step 3: Implement `gen-notices.sh`**: `header.md` + `cargo about generate --manifest-path Cargo.toml -c packaging/about.toml packaging/about.hbs` for the workspace (excluding `tools/uidrive`).
- [ ] **Step 4: Run** `tools/gen-notices.sh && tools/gen-notices.sh --check` — Expected: exit 0, and `THIRD_PARTY_NOTICES.md` names `ffmpeg-next`, `egui`, `reqwest` and `russh`. Then `cargo deny check` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(packaging): THIRD_PARTY_NOTICES generated with cargo-about, licence texts`.

### Task 4: Release gate `verify-appdir.sh`

**Files:**
- Create: `packaging/verify-appdir.sh`
- Modify: `tools/test-packaging.sh` (gate cases)

**Interfaces:**
- Consumes: the manifests from Task 2.
- Produces: `packaging/verify-appdir.sh <AppDir>` → prints one `PASS <check>`/`FAIL <check>: <detail>` line per check, then `RESULT: PASS|FAIL`; exit 0/1. Checks:
  1. `files`: `AppRun`, `space.kalopsia.fthrust.desktop`, `space.kalopsia.fthrust.png`, `.DirIcon`, `LICENSE`, `THIRD_PARTY_NOTICES.md`, `licenses/`, `usr/bin/{fthrust,fthrust-engine,fthrust-uploader}`, `usr/bin/assets/fonts/Oswald-Regular.ttf`.
  2. `ffmpeg-libs`: every `usr/lib/lib{av,sw,postproc}*` regular file is a key of `shipped_files_sha256` with the matching hash, all seven are present, and each soname in `soname_map` exists as a symlink to its file.
  3. `forbidden-flags`: no ELF under the AppDir contains any manifest `forbidden_flags` string.
  4. `linkage`: each of the three binaries has `DT_RPATH` exactly `$ORIGIN/../lib`, no `DT_RUNPATH`, and every `libav*/libsw*` NEEDED is a `soname_map` value; no `libpipewire*` file anywhere in the AppDir.
  5. `glibc`: the highest `GLIBC_x.y` across all ELF files (via `readelf --version-info --wide`) ≤ the baseline `max_glibc`; zero ELF files found → FAIL `no ELF files found`.
  The argument must be a directory (a file → `FAIL input: <path> is not a directory (extract the AppImage first)`).

- [ ] **Step 1: Write the failing tests** in `tools/test-packaging.sh gate`, using a fixture AppDir built in a temp dir from the fetched FFmpeg libs and a tiny C program compiled with `cc` as each of the three binaries (patchelf'd with `--force-rpath --set-rpath '$ORIGIN/../lib'`, linked with `-L$CACHE/ffmpeg/lib -lavutil`): `gate_passes_fixture`; `gate_fails_tampered_lib` (append a byte to `libavutil.so.60.26.102` → `FAIL ffmpeg-libs`); `gate_fails_runpath` (`patchelf --set-rpath` without `--force-rpath` → `FAIL linkage`); `gate_fails_forbidden_flag` (append `--enable-gpl` to `fthrust-uploader` → `FAIL forbidden-flags`); `gate_fails_bundled_pipewire` (touch `usr/lib/libpipewire-0.3.so.0` → `FAIL linkage`); `gate_rejects_file_input` (pass the fixture's `AppRun`); `gate_fails_empty_dir`. Glibc-ceiling failure: `gate_fails_glibc` runs the gate with `MAX_GLIBC_OVERRIDE=2.0`.
- [ ] **Step 2: Run** `tools/test-packaging.sh gate` — Expected: FAIL (script missing).
- [ ] **Step 3: Implement** `verify-appdir.sh` (bash + jq + readelf + grep -a). `MAX_GLIBC_OVERRIDE` env replaces the baseline for tests only. Compare versions with `sort -V`.
- [ ] **Step 4: Run** `tools/test-packaging.sh` (all cases) — Expected: all `ok`.
- [ ] **Step 5: Commit** `feat(packaging): release gate for AppDir contents, FFmpeg pins, linkage and glibc`.

### Task 5: `build-appimage.sh`, container build and version gate

**Files:**
- Create: `packaging/AppRun`, `packaging/Dockerfile`, `packaging/docker-build.sh`, `packaging/build-appimage.sh`, `tools/check-version.sh`
- Modify: `Cargo.toml` (add `[profile.release] strip = "debuginfo"`)

**Interfaces:**
- Consumes: fetcher (Task 2), notices (Task 3), gate (Task 4), desktop file and icon (Task 1).
- Produces: `packaging/build-appimage.sh [--skip-build]` → `dist/FTHRust-$VERSION-x86_64.AppImage` + `.sha256`, `dist/AppDir/` kept for inspection. `packaging/docker-build.sh` builds `packaging/Dockerfile` as image `fthrust-appimage-build` and runs `build-appimage.sh` inside with the repo mounted at `/src`, `CARGO_TARGET_DIR=/src/target/appimage` and a named volume for the cargo registry; the outputs end up in the host's `dist/`. `tools/check-version.sh [--tag REF] [--binary PATH]...` → exit 1 with `version mismatch: <where> has <x>, Cargo.toml has <y>`.
- `AppRun`: `HERE="$(dirname "$(readlink -f "$0")")"; exec "$HERE/usr/bin/fthrust" "$@"`.

- [ ] **Step 1: Write the Dockerfile** — `ubuntu:22.04`, apt packages `ca-certificates curl build-essential pkg-config clang libclang-dev meson ninja-build libwayland-dev libxkbcommon-dev patchelf file jq xz-utils desktop-file-utils squashfs-tools`, rustup with the `rust-toolchain.toml` channel, and `cargo install cargo-about --locked --version $CARGO_ABOUT_VERSION`.
- [ ] **Step 2: Implement `build-appimage.sh`**: read `VERSION` with `cargo metadata --no-deps --format-version 1 | jq`; run the fetcher and `build-pipewire-sdk.sh`; `PKG_CONFIG_PATH=$CACHE/ffmpeg/lib/pkgconfig:$CACHE/pipewire-sdk/lib/pkgconfig cargo build --release --locked -p fthrust-ui -p fthrust-engine -p fthrust-uploader`; assemble `dist/AppDir` (binaries and `assets/` into `usr/bin/`, the seven libs plus soname symlinks into `usr/lib/` with `cp -P`, desktop file and icon at the root and under `usr/share/applications/` and `usr/share/icons/hicolor/512x512/apps/`, `.DirIcon -> space.kalopsia.fthrust.png`, `LICENSE`, `licenses/`, `THIRD_PARTY_NOTICES.md` from `gen-notices.sh --out`); `patchelf --force-rpath --set-rpath '$ORIGIN/../lib'` on the three binaries; `desktop-file-validate`; `verify-appdir.sh dist/AppDir`; `ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 appimagetool --runtime-file runtime-x86_64 dist/AppDir <out>`; extract the result with `<out> --appimage-extract` into a temp dir and run `verify-appdir.sh` on `squashfs-root`; `tools/check-version.sh --binary squashfs-root/usr/bin/fthrust --binary …/fthrust-engine`; write the `.sha256`. On any gate failure delete the `.AppImage` and `.sha256`.
- [ ] **Step 3: Implement `check-version.sh`**: every `crates/*/Cargo.toml` uses `version.workspace = true` or equals the workspace version; `--tag refs/tags/vX` must equal `v$VERSION`; each `--binary` prints `<name> $VERSION` for `--version`.
- [ ] **Step 4: Run** `packaging/docker-build.sh` — Expected: `RESULT: PASS` twice (AppDir, extracted image) and both `dist/` files; `(cd dist && sha256sum -c FTHRust-*.sha256)` prints `OK`.
- [ ] **Step 5: Linkage check (Review Focus 1, 2):** run `LD_DEBUG=libs dist/AppDir/usr/bin/fthrust --version 2>&1` and the same for `fthrust-engine --version`; every `calling init:` line naming `libav`/`libsw` is under `dist/AppDir/usr/lib`. Add this as a final gate step in `build-appimage.sh` (on the extracted tree) so it is enforced, not just observed once.
- [ ] **Step 6: Commit** `feat(packaging): AppImage build in an Ubuntu 22.04 container with gates and version check`.

### Task 6: Live checks on the bench

**Files:**
- Modify: `tools/uidrive/README.md` (an "AppImage" section), `tools/uidrive/bench.sh` only if `--app` needs the engine path passed through

- [ ] **Step 1:** Extract the built image (`dist/FTHRust-*.AppImage --appimage-extract` into a scratch dir) and run `tools/uidrive/bench.sh run tools/uidrive/scenarios/smoke.sh --app <extracted>/AppRun --engine real`. Expected: the scenario passes and the screenshot matches the dev build (fonts and icons load from `usr/bin/assets`).
- [ ] **Step 2:** Run `save_clip.sh` and `export.sh` the same way. Expected: pass, and the engine log names `h264_vaapi` (the bundled libavutil dlopens the host libva).
- [ ] **Step 3:** Run the AppImage file itself (FUSE mount, not extracted) with `smoke.sh`. Expected: pass.
- [ ] **Step 4: Commit** any bench changes with `test(uidrive): run scenarios against the AppImage`.

### Task 7: CI and docs

**Files:**
- Create: `.github/workflows/release.yml`, `docs/packaging.md`
- Modify: `.github/workflows/ci.yml`, `README.md`

- [ ] **Step 1: `release.yml`** — triggers `push: tags: ['v*']` and `workflow_dispatch`; `permissions: contents: read`; job `appimage` on `ubuntu-22.04`: checkout, free disk (same `rm -rf` as `ci.yml`), install the Dockerfile's apt list, `dtolnay/rust-toolchain@1.98.1`, `cargo install cargo-about --locked --version …`, `tools/check-version.sh --tag "$GITHUB_REF"` when the ref is a tag, `packaging/build-appimage.sh`, upload `dist/FTHRust-*` as artifact `fthrust-appimage-${{ github.sha }}` (retention 7 days). No publish job.
- [ ] **Step 2: `ci.yml`** — add `permissions: contents: read`; in `check`, add `tools/check-assets.sh`, `tools/check-version.sh`, `tools/gen-notices.sh --check` (install cargo-about) and `tools/test-packaging.sh`.
- [ ] **Step 3: `docs/packaging.md`** — how to build locally (`packaging/docker-build.sh`), what the gates check, the host runtime dependencies, the app-id decision, and how to bump a pin (edit the manifest, run the fetcher, rerun `gen-notices.sh`). README gets a one-line pointer.
- [ ] **Step 4: Verify** with `actionlint` if installed, else a YAML parse (`python3 -c 'import yaml,sys; yaml.safe_load(open(sys.argv[1]))'`) of both workflows; push the branch and check that `ci` passes (`gh run watch`). Trigger `release.yml` with `gh workflow run release.yml --ref <branch>` and confirm the artifact appears.
- [ ] **Step 5: Commit** `ci: AppImage release job and packaging gates; docs: packaging`.
