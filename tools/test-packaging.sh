#!/usr/bin/env bash
# Tests for the packaging scripts. Usage: tools/test-packaging.sh [fetch|gate]...
# (no argument runs every group). The fetch group downloads the pinned
# AppImage runtime (~1 MB); the gate group needs the fetched FFmpeg tree,
# patchelf and cc.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FETCH="$ROOT/packaging/fetch-third-party.sh"
GATE="$ROOT/packaging/verify-appdir.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
failures=0

pass() { echo "ok $1"; }
fail() { echo "FAIL $1: $2"; failures=$((failures + 1)); }

# A private copy of the manifests that a test may edit.
manifests() {
    local dir="$WORK/manifests-$1"
    mkdir -p "$dir"
    cp "$ROOT"/packaging/*_manifest*.json "$dir/"
    echo "$dir"
}

edit_json() { # file jq-filter
    local tmp="$1.tmp"
    jq "$2" "$1" > "$tmp" && mv "$tmp" "$1"
}

rejects_http_url() {
    local m c out
    m="$(manifests http)"; c="$WORK/cache-http"
    edit_json "$m/appimage_tool_manifest.json" '.runtime.url |= sub("^https"; "http")'
    out="$("$FETCH" --appimagetool --manifest-dir "$m" --cache "$c" 2>&1)"
    if [ $? -eq 0 ]; then fail rejects_http_url "exit 0"; return; fi
    if ! grep -q https <<<"$out"; then fail rejects_http_url "message lacks https: $out"; return; fi
    if [ -n "$(find "$c" -type f 2>/dev/null)" ]; then fail rejects_http_url "cache not empty"; return; fi
    pass rejects_http_url
}

rejects_bad_sha_format() {
    local m c
    m="$(manifests sha)"; c="$WORK/cache-sha"
    edit_json "$m/appimage_tool_manifest.json" '.runtime.sha256 = "abc"'
    local out
    if out="$("$FETCH" --appimagetool --manifest-dir "$m" --cache "$c" 2>&1)"; then
        fail rejects_bad_sha_format "exit 0"; return
    fi
    if ! grep -q "fetch: .*sha256" <<<"$out"; then fail rejects_bad_sha_format "message: $out"; return; fi
    if [ -n "$(find "$c" -type f 2>/dev/null)" ]; then fail rejects_bad_sha_format "downloaded anyway"; return; fi
    pass rejects_bad_sha_format
}

refetches_tampered_cache() {
    local m c want got
    m="$(manifests tamper)"; c="$WORK/cache-tamper"
    # Runtime only: the appimagetool download is 15 MB, so drop it from this copy.
    edit_json "$m/appimage_tool_manifest.json" 'del(.appimagetool)'
    "$FETCH" --appimagetool --manifest-dir "$m" --cache "$c" >/dev/null 2>&1 || { fail refetches_tampered_cache "first fetch failed"; return; }
    printf x >> "$c/runtime-x86_64"
    "$FETCH" --appimagetool --manifest-dir "$m" --cache "$c" >/dev/null 2>&1 || { fail refetches_tampered_cache "second fetch failed"; return; }
    want="$(jq -r .runtime.sha256 "$m/appimage_tool_manifest.json")"
    got="$(sha256sum "$c/runtime-x86_64" | cut -d' ' -f1)"
    [ "$want" = "$got" ] && pass refetches_tampered_cache || fail refetches_tampered_cache "hash $got"
}

rejects_size_mismatch() {
    local m c
    m="$(manifests size)"; c="$WORK/cache-size"
    edit_json "$m/appimage_tool_manifest.json" 'del(.appimagetool) | .runtime.size += 1'
    local out
    if out="$("$FETCH" --appimagetool --manifest-dir "$m" --cache "$c" 2>&1)"; then
        fail rejects_size_mismatch "exit 0"; return
    fi
    if ! grep -q "fetch: .*size" <<<"$out"; then fail rejects_size_mismatch "message: $out"; return; fi
    if [ -n "$(find "$c" -type f 2>/dev/null)" ]; then fail rejects_size_mismatch "file left in cache"; return; fi
    pass rejects_size_mismatch
}

group_fetch() {
    rejects_http_url
    rejects_bad_sha_format
    refetches_tampered_cache
    rejects_size_mismatch
}

groups=("$@")
[ ${#groups[@]} -eq 0 ] && groups=(fetch gate)
for g in "${groups[@]}"; do
    if ! declare -F "group_$g" >/dev/null; then fail "group_$g" "no such group"; continue; fi
    "group_$g"
done
[ $failures -eq 0 ] && echo "all ok" || echo "$failures failed"
[ $failures -eq 0 ]
