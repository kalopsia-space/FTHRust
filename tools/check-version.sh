#!/usr/bin/env bash
# Version consistency: Cargo.toml [workspace.package].version is the single
# source; every crate inherits it, a release tag must be exactly v<version>,
# and each built binary must report it.
#
#   tools/check-version.sh [--tag REF] [--binary PATH]...
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml")"
[ -n "$VERSION" ] || { echo "version mismatch: no [workspace.package] version in Cargo.toml" >&2; exit 1; }
status=0
mismatch() { echo "version mismatch: $1 has $2, Cargo.toml has $VERSION" >&2; status=1; }

for manifest in "$ROOT"/crates/*/Cargo.toml; do
    v="$(sed -n '/^\[package\]/,/^\[/s/^version *= *\(.*\)/\1/p' "$manifest")"
    v="${v//\"/}"
    case "$v" in
        "$VERSION") ;;
        "") grep -q '^version.workspace = true' "$manifest" || mismatch "${manifest#"$ROOT"/}" "(none)" ;;
        *) mismatch "${manifest#"$ROOT"/}" "$v" ;;
    esac
done

while [ $# -gt 0 ]; do
    case "$1" in
        --tag)
            tag="${2#refs/tags/}"
            [ "$tag" = "v$VERSION" ] || mismatch tag "${tag#v}"
            shift ;;
        --binary)
            out="$("$2" --version 2>/dev/null || true)"
            got="$(awk '{print $2}' <<<"$out")"
            [ "$got" = "$VERSION" ] || mismatch "$2" "${got:-(no --version output)}"
            shift ;;
        *) echo "check-version: unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done
[ $status -eq 0 ] && echo "version $VERSION consistent"
exit $status
