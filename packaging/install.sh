#!/usr/bin/env bash
# Installs a release build linked against the system FFmpeg and PipeWire.
# Usage: packaging/install.sh [--prefix DIR] [--no-build] [--uninstall]
#   --prefix DIR   install root (default $PREFIX, else /usr/local)
#   --no-build     install the existing target/release binaries
#   --uninstall    remove what an install to the same prefix put there
# $DESTDIR is prepended to every path written, for distro packaging.
#
# Layout:
#   <prefix>/bin/fthrust, fthrust-engine, fthrust-uploader
#   <prefix>/share/fthrust/assets/
#   <prefix>/share/applications/space.kalopsia.fthrust.desktop
#   <prefix>/share/icons/hicolor/512x512/apps/space.kalopsia.fthrust.png
#   <prefix>/share/licenses/fthrust/
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG="$ROOT/packaging"
APP_ID=space.kalopsia.fthrust
BINS=(fthrust fthrust-engine fthrust-uploader)

prefix="${PREFIX:-/usr/local}"
build=1
uninstall=0
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        --no-build) build=0; shift ;;
        --uninstall) uninstall=1; shift ;;
        *) echo "install: unknown argument: $1" >&2; exit 2 ;;
    esac
done
case "$prefix" in
    /*) ;;
    *) echo "install: --prefix must be absolute: $prefix" >&2; exit 2 ;;
esac
prefix="${prefix%/}"
dest="${DESTDIR:-}$prefix"

bin_dir="$dest/bin"
share_dir="$dest/share/fthrust"
desktop="$dest/share/applications/$APP_ID.desktop"
icon="$dest/share/icons/hicolor/512x512/apps/$APP_ID.png"
licenses="$dest/share/licenses/fthrust"

if [ $uninstall = 1 ]; then
    for b in "${BINS[@]}"; do rm -f "$bin_dir/$b"; done
    rm -rf "$share_dir" "$licenses"
    rm -f "$desktop" "$icon"
    echo "removed FTHRust Clips from $dest"
    exit 0
fi

if [ $build = 1 ]; then
    if [ "$(id -u)" = 0 ]; then
        echo "install: not building as root; run 'cargo build --release --locked -p fthrust-ui -p fthrust-engine -p fthrust-uploader' as yourself, then this script with --no-build" >&2
        exit 1
    fi
    (cd "$ROOT" && cargo build --release --locked -p fthrust-ui -p fthrust-engine -p fthrust-uploader)
fi

target="${CARGO_TARGET_DIR:-$ROOT/target}/release"
for b in "${BINS[@]}"; do
    [ -x "$target/$b" ] || { echo "install: $target/$b is missing; build first or drop --no-build" >&2; exit 1; }
done

# Replace whole trees so files dropped upstream don't linger.
rm -rf "$share_dir" "$licenses"
mkdir -p "$bin_dir" "$share_dir" "$licenses" "$(dirname "$desktop")" "$(dirname "$icon")"
for b in "${BINS[@]}"; do install -m 755 "$target/$b" "$bin_dir/$b"; done
cp -r "$ROOT/assets" "$share_dir/assets"
# Absolute Exec so the launcher works when <prefix>/bin isn't on PATH.
sed "s|^Exec=fthrust|Exec=$prefix/bin/fthrust|" "$PKG/$APP_ID.desktop" > "$desktop"
chmod 644 "$desktop"
install -m 644 "$PKG/$APP_ID.png" "$icon"
install -m 644 "$ROOT/LICENSE" "$ROOT/THIRD_PARTY_NOTICES.md" "$licenses/"
cp -r "$ROOT/licenses" "$licenses/licenses"

echo "installed FTHRust Clips to $dest"
if [ -z "${DESTDIR:-}" ]; then
    case ":$PATH:" in
        *":$prefix/bin:"*) ;;
        *) echo "note: $prefix/bin is not on PATH; the desktop launcher still works" ;;
    esac
fi
