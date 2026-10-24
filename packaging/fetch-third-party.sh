#!/usr/bin/env bash
# Fetches the pinned third-party inputs of the AppImage build into the cache,
# checking each download's size and sha256 against packaging/*_manifest*.json.
#
#   packaging/fetch-third-party.sh [--ffmpeg] [--appimagetool] [--pipewire]
#                                  [--cache DIR] [--manifest-dir DIR]
#
# With no component flag every component is fetched. Results:
#   $CACHE/ffmpeg/                       BtbN LGPL tree (lib/, include/, lib/pkgconfig/)
#   $CACHE/appimagetool-x86_64.AppImage  (mode 755)
#   $CACHE/runtime-x86_64
#   $CACHE/pipewire-<version>.tar.gz     source for build-pipewire-sdk.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CACHE="$HERE/.cache"
MANIFESTS="$HERE"
want_ffmpeg=0 want_tools=0 want_pipewire=0

die() { echo "fetch: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --ffmpeg) want_ffmpeg=1 ;;
        --appimagetool) want_tools=1 ;;
        --pipewire) want_pipewire=1 ;;
        --cache) CACHE="$2"; shift ;;
        --manifest-dir) MANIFESTS="$2"; shift ;;
        *) die "unknown argument $1" ;;
    esac
    shift
done
if [ $((want_ffmpeg + want_tools + want_pipewire)) -eq 0 ]; then
    want_ffmpeg=1 want_tools=1 want_pipewire=1
fi

# validate NAME URL SIZE SHA256: refuse a bad pin before touching the network.
validate() {
    [[ "$2" == https://* ]] || die "$1: url must be https: $2"
    [[ "$3" =~ ^[0-9]+$ ]] || die "$1: size must be a byte count: $3"
    [[ "$4" =~ ^[0-9a-f]{64}$ ]] || die "$1: sha256 must be 64 lowercase hex digits: $4"
}

matches() { # FILE SIZE SHA256
    [ -f "$1" ] && [ "$(stat -c %s "$1")" = "$2" ] &&
        [ "$(sha256sum "$1" | cut -d' ' -f1)" = "$3" ]
}

# fetch NAME URL SIZE SHA256 DEST: reuse a verified cached copy, else
# download to DEST.part, check size then sha256, and move into place.
fetch() {
    local name="$1" url="$2" size="$3" sha="$4" dest="$5"
    validate "$name" "$url" "$size" "$sha"
    if matches "$dest" "$size" "$sha"; then
        echo "cached $dest"
        return
    fi
    rm -f "$dest"
    mkdir -p "$(dirname "$dest")"
    curl -fsSL --proto '=https' --tlsv1.2 -o "$dest.part" "$url" || { rm -f "$dest.part"; die "$name: download failed: $url"; }
    local got
    got="$(stat -c %s "$dest.part")"
    [ "$got" = "$size" ] || { rm -f "$dest.part"; die "$name: size $got, expected $size"; }
    got="$(sha256sum "$dest.part" | cut -d' ' -f1)"
    [ "$got" = "$sha" ] || { rm -f "$dest.part"; die "$name: sha256 $got, expected $sha"; }
    mv "$dest.part" "$dest"
    echo "fetched $dest"
}

# Every requested pin is checked before the first download.
if [ $want_tools = 1 ]; then
    m="$MANIFESTS/appimage_tool_manifest.json"
    [ -f "$m" ] || die "missing $m"
    for key in appimagetool runtime; do
        jq -e "has(\"$key\")" "$m" >/dev/null || continue
        validate "$key" "$(jq -r ".$key.url" "$m")" "$(jq -r ".$key.size" "$m")" "$(jq -r ".$key.sha256" "$m")"
    done
fi
if [ $want_pipewire = 1 ]; then
    m="$MANIFESTS/pipewire_sdk_manifest.json"
    [ -f "$m" ] || die "missing $m"
    validate pipewire "$(jq -r .url "$m")" "$(jq -r .size "$m")" "$(jq -r .sha256 "$m")"
fi
if [ $want_ffmpeg = 1 ]; then
    m="$MANIFESTS/ffmpeg_manifest_linux.json"
    [ -f "$m" ] || die "missing $m"
    validate ffmpeg "$(jq -r .source.url "$m")" "$(jq -r .source.size_bytes "$m")" "$(jq -r .source.sha256 "$m")"
    while read -r file sha; do
        [[ "$sha" =~ ^[0-9a-f]{64}$ ]] || die "ffmpeg: sha256 of $file must be 64 lowercase hex digits"
    done < <(jq -r '.shipped_files_sha256 | to_entries[] | "\(.key) \(.value)"' "$m")
fi

if [ $want_tools = 1 ]; then
    m="$MANIFESTS/appimage_tool_manifest.json"
    [ -f "$m" ] || die "missing $m"
    for key in appimagetool runtime; do
        jq -e "has(\"$key\")" "$m" >/dev/null || continue
        file="appimagetool-x86_64.AppImage"
        [ $key = runtime ] && file="runtime-x86_64"
        fetch "$key" "$(jq -r ".$key.url" "$m")" "$(jq -r ".$key.size" "$m")" \
            "$(jq -r ".$key.sha256" "$m")" "$CACHE/$file"
        [ $key = appimagetool ] && chmod 755 "$CACHE/$file"
    done
fi

if [ $want_pipewire = 1 ]; then
    m="$MANIFESTS/pipewire_sdk_manifest.json"
    [ -f "$m" ] || die "missing $m"
    fetch pipewire "$(jq -r .url "$m")" "$(jq -r .size "$m")" "$(jq -r .sha256 "$m")" \
        "$CACHE/pipewire-$(jq -r .version "$m").tar.gz"
fi

if [ $want_ffmpeg = 1 ]; then
    m="$MANIFESTS/ffmpeg_manifest_linux.json"
    [ -f "$m" ] || die "missing $m"
    archive="$CACHE/$(jq -r .source.asset "$m")"
    fetch ffmpeg "$(jq -r .source.url "$m")" "$(jq -r .source.size_bytes "$m")" \
        "$(jq -r .source.sha256 "$m")" "$archive"
    tree="$CACHE/ffmpeg"
    ok=1
    while read -r file sha; do
        [ "$(sha256sum "$tree/lib/$file" 2>/dev/null | cut -d' ' -f1)" = "$sha" ] || ok=0
    done < <(jq -r '.shipped_files_sha256 | to_entries[] | "\(.key) \(.value)"' "$m")
    if [ $ok = 1 ] && [ ! -e "$tree/bin" ]; then
        echo "cached $tree"
    else
        rm -rf "$tree" "$CACHE/ffmpeg.tmp"
        mkdir -p "$CACHE/ffmpeg.tmp"
        tar -xJf "$archive" -C "$CACHE/ffmpeg.tmp"
        mv "$CACHE"/ffmpeg.tmp/ffmpeg-* "$tree"
        rm -rf "$CACHE/ffmpeg.tmp" "$tree/bin"
        while read -r file sha; do
            got="$(sha256sum "$tree/lib/$file" 2>/dev/null | cut -d' ' -f1)"
            [ "$got" = "$sha" ] || { rm -rf "$tree"; die "ffmpeg: lib/$file sha256 ${got:-missing}, expected $sha"; }
        done < <(jq -r '.shipped_files_sha256 | to_entries[] | "\(.key) \(.value)"' "$m")
        echo "extracted $tree"
    fi
fi
