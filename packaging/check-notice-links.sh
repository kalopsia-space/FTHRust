#!/usr/bin/env bash
# Fails when a relative markdown link points at a file that is not there.
#
#   packaging/check-notice-links.sh [--require-links] FILE...
#
# THIRD_PARTY_NOTICES.md links to LICENSE, licenses/ and the FFmpeg manifest,
# and the same links have to work in the repository, the AppImage, the tarball
# (twice: at its top and under share/licenses/fthrust/) and the native install,
# so this runs on each copy. It also serves the README, CHANGES.md and docs/.
# --require-links makes a file with no relative link at all an error, so a
# broken link extraction cannot pass vacuously.
#
# Only the part before "## Rust crates" is read: what follows is licence text,
# which may contain anything that looks like a link. Anchors are not checked.
set -euo pipefail
require=0
if [ "${1:-}" = "--require-links" ]; then require=1; shift; fi
[ $# -gt 0 ] || { echo "usage: check-notice-links.sh [--require-links] FILE..." >&2; exit 2; }
status=0
for f in "$@"; do
    [ -f "$f" ] || { echo "check-notice-links: $f is not a file" >&2; status=1; continue; }
    dir="$(dirname "$f")"
    checked=0
    while IFS= read -r target; do
        case "$target" in http://* | https://* | mailto:* | '#'*) continue ;; esac
        path="${target%%#*}"
        [ -n "$path" ] || continue
        checked=$((checked + 1))
        [ -e "$dir/$path" ] || { echo "check-notice-links: $f links to '$target', which is not there" >&2; status=1; }
    done < <(sed '/^## Rust crates$/,$d' "$f" | grep -o '\]([^)[:space:]]*)' | sed 's/^](//; s/)$//' || true)
    if [ $require = 1 ] && [ $checked -eq 0 ]; then
        echo "check-notice-links: $f has no relative links to check" >&2
        status=1
    fi
done
exit $status
