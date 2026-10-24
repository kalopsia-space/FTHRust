#!/usr/bin/env bash
# Builds the PipeWire headers and link library the workspace compiles against
# into $CACHE/pipewire-sdk. Build only: the AppImage never ships PipeWire; the
# binaries load the host's libpipewire-0.3.so.0. Needed because the Ubuntu
# 22.04 release baseline ships PipeWire 0.3.48, older than libspa 0.10 needs.
#
#   packaging/build-pipewire-sdk.sh [--cache DIR]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CACHE="$HERE/.cache"
while [ $# -gt 0 ]; do
    case "$1" in
        --cache) CACHE="$2"; shift ;;
        *) echo "build-pipewire-sdk: unknown argument $1" >&2; exit 1 ;;
    esac
    shift
done
VERSION="$(jq -r .version "$HERE/pipewire_sdk_manifest.json")"
SDK="$CACHE/pipewire-sdk"
if [ "$(cat "$SDK/.version" 2>/dev/null)" = "$VERSION" ]; then
    echo "cached $SDK"
    exit 0
fi
"$HERE/fetch-third-party.sh" --pipewire --cache "$CACHE" >/dev/null
SRC="$CACHE/pipewire-src"
rm -rf "$SRC" "$SDK"
mkdir -p "$SRC"
tar -xzf "$CACHE/pipewire-$VERSION.tar.gz" -C "$SRC" --strip-components=1
meson setup "$SRC/build" "$SRC" --prefix="$SDK" --libdir=lib \
    -Dauto_features=disabled -Dflatpak=disabled -Dsession-managers=[] \
    -Dexamples=disabled -Dtests=disabled >/dev/null
ninja -C "$SRC/build" install >/dev/null
rm -rf "$SRC"
echo "$VERSION" > "$SDK/.version"
echo "built $SDK"
