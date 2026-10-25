#!/usr/bin/env bash
# Builds dist/FTHRust-<version>-x86_64.AppImage, the portable
# dist/FTHRust-<version>-x86_64.tar.gz (the same binaries and FFmpeg as a
# relocatable prefix) and their .sha256 files on an Ubuntu 22.04 host (CI) or
# inside packaging/docker-build.sh (anywhere else).
#
#   packaging/build-appimage.sh [--skip-build]
#
# Gates run on the AppDir before packing and on the extracted image after;
# nothing reaches dist/ unless every gate passes. dist/AppDir is kept for
# inspection.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG="$ROOT/packaging"
CACHE="$PKG/.cache"
DIST="$ROOT/dist"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
APP_ID=space.kalopsia.fthrust
skip_build=0
[ "${1:-}" = "--skip-build" ] && skip_build=1

step() { echo "==> $*"; }
die() { echo "build-appimage: $*" >&2; exit 1; }

cd "$ROOT"
VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "fthrust-ui") | .version')"
[ -n "$VERSION" ] || die "could not read the workspace version"
NAME="FTHRust-$VERSION-x86_64.AppImage"
TARBASE="FTHRust-$VERSION-x86_64"
TARNAME="$TARBASE.tar.gz"
# An earlier build of this version must not look like this build's output.
rm -f "$DIST/$NAME" "$DIST/$NAME.sha256" "$DIST/$TARNAME" "$DIST/$TARNAME.sha256"

step "fetching pinned inputs"
"$PKG/fetch-third-party.sh" --cache "$CACHE"
"$PKG/build-pipewire-sdk.sh" --cache "$CACHE"
PW_LIB="$CACHE/pipewire-sdk/lib"

if [ $skip_build = 0 ]; then
    step "building release binaries against FFmpeg $(jq -r .version "$PKG/ffmpeg_manifest_linux.json")"
    PKG_CONFIG_PATH="$CACHE/ffmpeg/lib/pkgconfig:$PW_LIB/pkgconfig" \
        cargo build --release --locked -p fthrust-ui -p fthrust-engine -p fthrust-uploader
fi

step "checking notices"
"$ROOT/tools/gen-notices.sh" --check

step "assembling AppDir"
APPDIR="$DIST/AppDir"
rm -rf "$APPDIR" "$DIST/.staging"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/512x512/apps" "$DIST/.staging"
for b in fthrust fthrust-engine fthrust-uploader; do
    install -m 755 "$TARGET/release/$b" "$APPDIR/usr/bin/$b"
    patchelf --force-rpath --set-rpath '$ORIGIN/../lib' "$APPDIR/usr/bin/$b"
done
cp -r "$ROOT/assets" "$APPDIR/usr/bin/assets"
while read -r file soname; do
    cp "$CACHE/ffmpeg/lib/$file" "$APPDIR/usr/lib/$file"
    ln -s "$file" "$APPDIR/usr/lib/$soname"
done < <(jq -r '.soname_map | to_entries[] | "\(.key) \(.value)"' "$PKG/ffmpeg_manifest_linux.json")
install -m 755 "$PKG/AppRun" "$APPDIR/AppRun"
cp "$PKG/$APP_ID.desktop" "$APPDIR/"
cp "$PKG/$APP_ID.desktop" "$APPDIR/usr/share/applications/"
cp "$PKG/$APP_ID.png" "$APPDIR/"
cp "$PKG/$APP_ID.png" "$APPDIR/usr/share/icons/hicolor/512x512/apps/"
ln -s "$APP_ID.png" "$APPDIR/.DirIcon"
cp "$ROOT/LICENSE" "$ROOT/THIRD_PARTY_NOTICES.md" "$APPDIR/"
cp -r "$ROOT/licenses" "$APPDIR/licenses"
desktop-file-validate "$APPDIR/$APP_ID.desktop"

step "gating AppDir"
"$PKG/verify-appdir.sh" "$APPDIR"

step "packing $NAME"
OUT="$DIST/.staging/$NAME"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$CACHE/appimagetool-x86_64.AppImage" \
    --no-appstream --runtime-file "$CACHE/runtime-x86_64" "$APPDIR" "$OUT"

step "gating the extracted image"
EXTRACT="$DIST/.staging/extract"
mkdir -p "$EXTRACT"
(cd "$EXTRACT" && "$OUT" --appimage-extract >/dev/null)
ROOTFS="$EXTRACT/squashfs-root"
"$PKG/verify-appdir.sh" "$ROOTFS"
# The host's libpipewire is absent from the build image; LD_LIBRARY_PATH only
# supplies it, because DT_RPATH outranks LD_LIBRARY_PATH for FFmpeg.
gate_ffmpeg_loads() { # <root with bin/ and lib/>
    local root="$1" b log loaded stray
    for b in fthrust fthrust-engine; do
        log="$(LD_LIBRARY_PATH="$PW_LIB" LD_DEBUG=libs "$root/bin/$b" --version 2>&1 >/dev/null)"
        loaded="$(grep -oE 'calling init: \S*lib(av|sw)\S*' <<<"$log" | sed 's/calling init: //' | xargs -r readlink -f)"
        [ -n "$loaded" ] || die "$b loaded no FFmpeg library"
        stray="$(grep -vF "$(readlink -f "$root/lib")/" <<<"$loaded" || true)"
        [ -z "$stray" ] || die "$b loaded FFmpeg from outside the package: $stray"
        echo "PASS $b loads FFmpeg from ${root#"$DIST/.staging/"}/lib ($(wc -l <<<"$loaded") libraries)"
    done
    LD_LIBRARY_PATH="$PW_LIB" "$ROOT/tools/check-version.sh" \
        --binary "$root/bin/fthrust" --binary "$root/bin/fthrust-engine"
}
gate_ffmpeg_loads "$ROOTFS/usr"

step "packing $TARNAME"
# The gated AppDir's binaries and FFmpeg as a prefix: bin/ keeps its
# $ORIGIN/../lib rpath and the app finds share/fthrust/assets.
TREE="$DIST/.staging/tree/$TARBASE"
mkdir -p "$TREE/bin" "$TREE/share/fthrust" "$TREE/share/applications" \
    "$TREE/share/icons/hicolor/512x512/apps" "$TREE/share/licenses/fthrust"
for b in fthrust fthrust-engine fthrust-uploader; do
    cp -a "$APPDIR/usr/bin/$b" "$TREE/bin/$b"
done
cp -a "$APPDIR/usr/lib" "$TREE/lib"
cp -a "$APPDIR/usr/bin/assets" "$TREE/share/fthrust/assets"
cp "$PKG/$APP_ID.desktop" "$TREE/share/applications/"
cp "$PKG/$APP_ID.png" "$TREE/share/icons/hicolor/512x512/apps/"
cp "$ROOT/LICENSE" "$ROOT/THIRD_PARTY_NOTICES.md" "$TREE/"
cp -r "$ROOT/licenses" "$TREE/share/licenses/fthrust/licenses"
cp "$ROOT/LICENSE" "$ROOT/THIRD_PARTY_NOTICES.md" "$TREE/share/licenses/fthrust/"
TAROUT="$DIST/.staging/$TARNAME"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@${SOURCE_DATE_EPOCH:-0}" \
    -C "$DIST/.staging/tree" -cf - "$TARBASE" | gzip -n -9 > "$TAROUT"

step "gating the extracted tarball"
TEXTRACT="$DIST/.staging/textract"
mkdir -p "$TEXTRACT"
tar -C "$TEXTRACT" -xzf "$TAROUT"
[ -f "$TEXTRACT/$TARBASE/share/fthrust/assets/fonts/Oswald-Regular.ttf" ] || die "tarball lacks share/fthrust/assets"
[ -x "$TEXTRACT/$TARBASE/bin/fthrust-uploader" ] || die "tarball lacks bin/fthrust-uploader"
gate_ffmpeg_loads "$TEXTRACT/$TARBASE"

step "writing $NAME.sha256 and $TARNAME.sha256"
mv "$OUT" "$DIST/$NAME"
mv "$TAROUT" "$DIST/$TARNAME"
rm -rf "$DIST/.staging"
(cd "$DIST" && sha256sum "$NAME" > "$NAME.sha256" && sha256sum "$TARNAME" > "$TARNAME.sha256" \
    && cat "$NAME.sha256" "$TARNAME.sha256")
