# Packaging

FTHRust Clips is released as an AppImage built on Ubuntu 22.04, so it runs on any x86_64 Linux with glibc 2.35 or newer. This page covers what goes into it, how to build it, the gates it has to pass, and how a release is made.

- [What's in the AppImage](#whats-in-the-appimage)
- [Building it](#building-it)
- [The build, step by step](#the-build-step-by-step)
- [Gates](#gates)
- [Pinned inputs](#pinned-inputs)
- [Licences and notices](#licences-and-notices)
- [Releasing](#releasing)
- [Native install](#native-install)

## What's in the AppImage

```
AppRun                          execs usr/bin/fthrust with your arguments
space.kalopsia.fthrust.desktop  desktop entry (also in usr/share/applications)
space.kalopsia.fthrust.png      icon (also in usr/share/icons/hicolor/512x512)
LICENSE, THIRD_PARTY_NOTICES.md, licenses/
packaging/ffmpeg_manifest_linux.json   the FFmpeg manifest the notices link to
usr/bin/fthrust                 the app
usr/bin/fthrust-engine          the capture engine
usr/bin/fthrust-uploader        the uploader
usr/bin/assets/                 fonts, icons, sounds
usr/lib/libav*.so.*, libsw*.so.* a pinned FFmpeg 8.1 (BtbN's lgpl-shared build)
```

The release tarball `FTHRust-<version>-x86_64.tar.gz` packs the same gated files as a relocatable prefix: `bin/` (the three binaries, rpath unchanged), `lib/` (FFmpeg), `share/fthrust/assets/`, `share/applications/`, `share/icons/`, `share/licenses/fthrust/`, plus `LICENSE`, `THIRD_PARTY_NOTICES.md`, `licenses/` and `packaging/ffmpeg_manifest_linux.json` at the top. The notices and licence texts are in both places, so the links in `THIRD_PARTY_NOTICES.md` resolve next to either copy. `build-appimage.sh` builds it after the AppImage, extracts it, and checks that `bin/fthrust` and `bin/fthrust-engine` load FFmpeg only from the tarball's `lib/` and report the workspace version.

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

`packaging/verify-appdir.sh <AppDir>` prints `PASS` or `FAIL` for each of eight checks and a final `RESULT:`:

1. **files**: every required file is present and the three binaries are executable.
2. **ffmpeg-libs**: the FFmpeg libraries are exactly the pinned ones, unmodified.
3. **forbidden-flags**: FFmpeg was configured without `--enable-gpl`, `--enable-nonfree`, x264, x265, xvid or xavs2. That makes it an LGPL build of FFmpeg, not an LGPL bundle: a library BtbN's scripts compile in for one of the enabled ones can still be GPL (gate 8).
4. **linkage**: binaries use an `RPATH` (not `RUNPATH`) to `usr/lib`, link only the pinned FFmpeg sonames, and **don't bundle `libpipewire`**.
5. **glibc ceiling**: nothing needs a glibc newer than **2.35** (the baseline in `linux_release_baseline.json`).
6. **ffmpeg-config**: every bundled FFmpeg library contains the configure line the manifest records. The licence notices are generated from that line, so this keeps them tied to the binary.
7. **notice-links**: every relative link in `THIRD_PARTY_NOTICES.md` resolves inside the package (`packaging/check-notice-links.sh`).
8. **gpl-libs**: a library the registry marks `"gpl": true` (FFTW, built into `libavformat` through Chromaprint) contains its signature string, and exactly the bundled libraries the registry names do. A mismatch means the licence notices describe another build than the package.

`tools/test-packaging.sh` tests the gates themselves, by feeding them a good AppDir and deliberately broken ones (a tampered library, a `RUNPATH` instead of an `RPATH`, and so on). It needs `patchelf` and the fetched FFmpeg, so it's easiest to run through `docker-build.sh`.

## Pinned inputs

| Input | Pinned in | Notes |
|---|---|---|
| FFmpeg | `ffmpeg_manifest_linux.json` | BtbN's `lgpl-shared` build of FFmpeg 8.1, with its sonames, forbidden configure flags, the build's configure line, and the commits of FFmpeg and of BtbN's build scripts that it was built from (`corresponding_source`). |
| FFmpeg libraries | `ffmpeg_libraries.json`, `../licenses/ffmpeg/` | What each external library in the configure line is, its licence and licence text. See [Licences and notices](#licences-and-notices). |
| appimagetool and runtime | `appimage_tool_manifest.json` | appimagetool 1.9.1 and the type-2 runtime. |
| PipeWire SDK | `pipewire_sdk_manifest.json` | PipeWire 1.0.5 source, for linking only. |
| Platform baseline | `linux_release_baseline.json` | x86_64, Ubuntu 22.04, glibc 2.35 at most. |
| Rust | `rust-toolchain.toml` | Also passed into the build image. |

To update an input, change its manifest (URL, size and SHA-256), rebuild, and let the gates tell you what broke.

### Updating FFmpeg

A new BtbN build changes more than a URL:

1. In `ffmpeg_manifest_linux.json` set the release, asset, size and hash, the sonames and library hashes, `configuration` (the configure line, printed by `strings libavutil.so.* | grep -- --prefix=`; the gate fails until it matches the libraries) and `corresponding_source`: the FFmpeg commit from the version string (`n8.1.2-34-g9b6c8969e0` is 34 commits after `n8.1.2`), and the commit of the BtbN scheduled run that published the asset (**Actions → Build FFmpeg**, its head SHA).
2. Run `packaging/check-ffmpeg-depends.py` (it fetches BtbN's scripts at the commit you just pinned, and retries a failed fetch). It follows `ffbuild_depends` from every enabled option, and a script brings the other scripts in its directory (BtbN's final script depends on whole directories). It fails for each repository such a script builds that the registry neither lists nor records under `not_linked` with the reason none of it is in the shipped libraries, for a pin that differs from what the script builds, for a `not_linked` entry nothing needs, and for a `supporting_libraries` entry that no enabled option reaches any more, so dropping an option fails in the pull request that does it. A new transitive library is either added to `supporting_libraries` (with `"gpl": true`, a `signature` string and `in_libraries` if it has no non-GPL licence option: look for the string in the new libraries with `strings`) or goes under `not_linked` after checking the libraries for traces of it.
3. Run `tools/gen-notices.sh`. It fails while the new configure line has an option `ffmpeg_libraries.json` does not describe, or the registry describes one it no longer has, or a licence text is missing or unused. For a new library add its entry (the repository and commit are in BtbN's `scripts.d/` at the commit you just pinned), put its licence file in `licenses/ffmpeg/`, and read the licence off the file.
4. `packaging/archive-ffmpeg-source.sh --out /tmp/ffmpeg-source.tar.xz` should work against the new pins; the release does the same, including the dependency check.

## Licences and notices

`THIRD_PARTY_NOTICES.md` is generated by `tools/gen-notices.sh` from three things:

- `packaging/notices/header.md`: the hand-written parts (the distribution licence summary, the FFmpeg overview, the AppImage runtime, the project assets and what is not distributed). The FFmpeg version, hashes and commits in it are placeholders filled from `ffmpeg_manifest_linux.json`.
- The FFmpeg libraries, written by `packaging/gen-ffmpeg-notice.py` from the manifest's configure line and `ffmpeg_libraries.json`: a table of every external library BtbN's build is configured with (and the supporting libraries its scripts build for them), each with its licence, upstream, the commit BtbN's scripts pin, and its licence text under `licenses/ffmpeg/`.
- [cargo-about](https://github.com/EmbarkStudios/cargo-about), for the licence of every crate linked into the shipped binaries. Dev and build dependencies are excluded. The accepted licences in `packaging/about.toml` mirror `deny.toml`.

The licences in the FFmpeg table are read off each project's own licence file; they are an inventory, not legal advice. The generator refuses to run when the configure line and the registry disagree. That only covers the libraries a configure option names: `check-ffmpeg-depends.py` (a CI step, and part of the release's source archive step) covers the ones BtbN's scripts pull in for them, which is how FFTW was found. It reads the scripts' declared dependencies and the directories they sit in, so a library linked in without either stays invisible, and `not_linked` entries rest on a look at the shipped libraries, not a proof. It fails the other way round as well: a supporting library whose script no enabled option reaches is an error, so the registry cannot go on describing a library the build dropped.

**One library in the package is GPL.** FFTW (GPL-2.0-or-later, no other licence option) is compiled into `libavformat` because BtbN builds Chromaprint against it. The registry marks it, the generated notices say so wherever they speak of FFmpeg's licence, gate 8 checks it against the libraries, and its source goes into the release's source archive. FFmpeg's own code, and the libraries without GPL code in them, stay LGPL. Building FFmpeg without `--enable-chromaprint` (the app does not use it) would remove FFTW.

FFmpeg is LGPL and `libavformat` carries a GPL library, so the release also carries their source. BtbN prunes monthly builds after about two years, so `fetch-third-party.sh --ffmpeg` cannot rebuild an old release; instead `packaging/archive-ffmpeg-source.sh` writes `FTHRust-ffmpeg-source-<version>.tar.xz` from the FFmpeg commit and the BtbN build-script commit pinned in the manifest and the pinned commit of each GPL library (all checked against their full commit ids), and the release workflow attaches it. The sources of the other external libraries are not in it: BtbN's `scripts.d/` names each one's repository and commit.

After changing dependencies or the FFmpeg pin:

```sh
tools/gen-notices.sh           # regenerate
tools/gen-notices.sh --check   # what CI runs
```

## Releasing

1. Bump `version` in the root `Cargo.toml`. It's the only place the version lives.
2. Add a `## <version>` section to `CHANGES.md` (`## 2.0.0-alpha.3`, optionally followed by a date). It becomes the body of the GitHub release; `packaging/release-notes.sh <tag>` prints it and fails when the section is missing or empty.
3. Commit, then tag `v<version>` (for example `v2.0.0-alpha.3`) and push the tag.
4. `.github/workflows/release.yml` takes it from there:
   - **notes** checks that the tag matches the workspace version and that `CHANGES.md` has the section, in seconds.
   - **ci** runs the whole [CI](development.md#ci) for the tag's commit. Nothing is built or published unless it is green.
   - **appimage** (Ubuntu 22.04) builds with `packaging/build-appimage.sh`, writes the FFmpeg source archive, verifies the checksums, signs a build provenance attestation for the AppImage and the tarball, and uploads everything as the `fthrust-appimage-<commit>` artifact, kept for 7 days.
   - **publish** creates the GitHub release for the tag with the AppImage, the tarball, the FFmpeg source archive and their `.sha256` files attached. The body is the `CHANGES.md` section, how to verify a download, and GitHub's compare link. A tag with a `-` in it (`v2.0.0-alpha.1`) is marked as a pre-release.

Anyone can check a download with `sha256sum -c` and `gh attestation verify FTHRust-<version>-x86_64.AppImage --repo kalopsia-space/FTHRust`.

The workflow can also be started by hand (**Actions → release → Run workflow**) to build from any branch without tagging. A hand-started run skips the notes and CI jobs, builds the artifact and publishes nothing.

## Native install

`packaging/install.sh` installs a release build linked against the system's FFmpeg and PipeWire, for people who don't want the AppImage and for distribution packages. It takes `--prefix DIR` (default `$PREFIX`, else `/usr/local`), `--no-build` and `--uninstall`, and puts `$DESTDIR` in front of every path it writes. It won't run `cargo` as root.

```
<prefix>/bin/fthrust, fthrust-engine, fthrust-uploader
<prefix>/share/fthrust/assets/                                  fonts, icons, sounds
<prefix>/share/applications/space.kalopsia.fthrust.desktop      Exec= points at <prefix>/bin/fthrust
<prefix>/share/icons/hicolor/512x512/apps/space.kalopsia.fthrust.png
<prefix>/share/licenses/fthrust/                                LICENSE, THIRD_PARTY_NOTICES.md, licenses/
```

The app looks for its assets in `<exe dir>/assets` (the AppImage layout), then `<exe dir>/../share/fthrust/assets` (this layout), then the source tree it was built from. `$FTHRUST_ASSETS` overrides all three. `tools/test-packaging.sh install` covers the script.
