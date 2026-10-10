# Linux x86_64 build environment for Dyktando X (used by scripts/ci.sh --linux and
# scripts/release.sh --linux). Mirrors the ubuntu-24.04 GitHub runner: same apt packages
# as .github/workflows/ci.yml, Node 22, Rust 1.96.
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git build-essential pkg-config file xz-utils \
      libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf \
      libasound2-dev libdbus-1-dev libxdo-dev libclang-dev cmake \
 && rm -rf /var/lib/apt/lists/*

ARG NODE_VERSION=22.22.0
RUN curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
      | tar -xJ -C /usr/local --strip-components=1 \
 && node --version && npm --version

ARG RUST_VERSION=1.96.0
ENV RUSTUP_HOME=/usr/local/rustup PATH=/root/.cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" \
 && rustc --version
