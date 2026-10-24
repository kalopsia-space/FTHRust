#!/usr/bin/env bash
# Runs a command (default: packaging/build-appimage.sh) inside the Ubuntu
# 22.04 release image, with the repository mounted at /src. Outputs land in
# the host's dist/ and target/appimage/.
#
#   packaging/docker-build.sh [COMMAND ARGS...]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLCHAIN="$(sed -n 's/^channel = "\(.*\)"/\1/p' "$ROOT/rust-toolchain.toml")"
ABOUT="$(sed -n 's/^CARGO_ABOUT_VERSION=//p' "$ROOT/tools/gen-notices.sh")"
docker build -q -t fthrust-appimage-build \
    --build-arg RUST_TOOLCHAIN="$TOOLCHAIN" --build-arg CARGO_ABOUT_VERSION="$ABOUT" \
    "$ROOT/packaging" >/dev/null
mkdir -p "$ROOT/target/appimage-registry"
[ $# -gt 0 ] || set -- packaging/build-appimage.sh
exec docker run --rm --user "$(id -u):$(id -g)" \
    -v "$ROOT:/src" -w /src \
    -v "$ROOT/target/appimage-registry:/opt/cargo/registry" \
    -e CARGO_TARGET_DIR=/src/target/appimage -e HOME=/tmp \
    fthrust-appimage-build "$@"
