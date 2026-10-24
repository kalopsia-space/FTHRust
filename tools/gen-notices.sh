#!/usr/bin/env bash
# Writes THIRD_PARTY_NOTICES.md: packaging/notices/header.md followed by the
# licences of every Rust crate linked into the shipped binaries.
#
#   tools/gen-notices.sh [--check] [--out FILE]
#
# --check regenerates into a temporary file and fails when it differs from
# the committed THIRD_PARTY_NOTICES.md (CI never rewrites tracked files).
set -euo pipefail
CARGO_ABOUT_VERSION=0.9.2
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/THIRD_PARTY_NOTICES.md"
check=0
while [ $# -gt 0 ]; do
    case "$1" in
        --check) check=1 ;;
        --out) OUT="$2"; shift ;;
        *) echo "gen-notices: unknown argument $1" >&2; exit 1 ;;
    esac
    shift
done
have="$(cargo about --version 2>/dev/null | awk '{print $2}' || true)"
if [ "$have" != "$CARGO_ABOUT_VERSION" ]; then
    echo "gen-notices: need cargo-about $CARGO_ABOUT_VERSION (have ${have:-none}): cargo install cargo-about --locked --features cli --version $CARGO_ABOUT_VERSION" >&2
    exit 1
fi
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
{
    cat "$ROOT/packaging/notices/header.md"
    echo
    (cd "$ROOT" && cargo about generate --locked --offline --fail -c packaging/about.toml packaging/about.hbs)
} > "$tmp"
if [ $check = 1 ]; then
    if ! cmp -s "$tmp" "$ROOT/THIRD_PARTY_NOTICES.md"; then
        echo "notices out of date: run tools/gen-notices.sh" >&2
        exit 1
    fi
    echo "notices up to date"
else
    cp "$tmp" "$OUT"
    echo "wrote $OUT"
fi
