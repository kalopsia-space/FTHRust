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
# The link library must come from the same glibc as the binaries it links.
STAMP="$VERSION $(getconf GNU_LIBC_VERSION)"
if [ "$(cat "$SDK/.version" 2>/dev/null)" = "$STAMP" ]; then
    echo "cached $SDK"
    exit 0
fi
"$HERE/fetch-third-party.sh" --pipewire --cache "$CACHE" >/dev/null
SRC="$CACHE/pipewire-src"
rm -rf "$SRC" "$SDK"
mkdir -p "$SRC"
tar -xzf "$CACHE/pipewire-$VERSION.tar.gz" -C "$SRC" --strip-components=1
log="$CACHE/pipewire-sdk-build.log"
if ! { meson setup "$SRC/build" "$SRC" --prefix="$SDK" --libdir=lib \
        -Dauto_features=disabled -Dflatpak=disabled -Ddbus=disabled \
        -Dsession-managers=[] -Dexamples=disabled -Dtests=disabled &&
    ninja -C "$SRC/build" install; } > "$log" 2>&1; then
    tail -20 "$log" >&2
    echo "build-pipewire-sdk: build failed (log: $log)" >&2
    exit 1
fi
rm -rf "$SRC"
# Relocatable: the cache is used from different mount points (host, /src).
sed -i 's|^prefix=.*|prefix=${pcfiledir}/../..|' "$SDK"/lib/pkgconfig/*.pc
echo "$STAMP" > "$SDK/.version"
echo "built $SDK"
