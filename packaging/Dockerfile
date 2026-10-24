# Release build image: Ubuntu 22.04 sets the glibc 2.35 baseline
# (packaging/linux_release_baseline.json). FFmpeg and the PipeWire link SDK
# come from the pinned manifests, never from apt.
FROM ubuntu:22.04
ARG RUST_TOOLCHAIN
ARG CARGO_ABOUT_VERSION
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl build-essential pkg-config clang libclang-dev \
        meson ninja-build libwayland-dev libxkbcommon-dev patchelf file jq \
        xz-utils desktop-file-utils squashfs-tools git \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf --proto '=https' --tlsv1.2 https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain "$RUST_TOOLCHAIN" -c rustfmt -c clippy \
    && cargo install cargo-about --locked --features cli --version "$CARGO_ABOUT_VERSION" \
    && chmod -R a+rwX /opt/cargo /opt/rustup
