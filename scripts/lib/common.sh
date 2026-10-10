# Shared helpers for scripts/*.sh (sourced, not executed).
# shellcheck shell=bash

if [[ -t 2 ]]; then
  _c_blue=$'\033[1;34m'; _c_yellow=$'\033[1;33m'; _c_red=$'\033[1;31m'; _c_off=$'\033[0m'
else
  _c_blue=''; _c_yellow=''; _c_red=''; _c_off=''
fi

log()  { printf '%s==>%s %s\n' "$_c_blue" "$_c_off" "$*" >&2; }
warn() { printf '%swarning:%s %s\n' "$_c_yellow" "$_c_off" "$*" >&2; }
die()  { printf '%serror:%s %s\n' "$_c_red" "$_c_off" "$*" >&2; exit 1; }

need() {
  local tool
  for tool in "$@"; do
    command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool"
  done
}

# Repository root (scripts/ lives directly under it).
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export REPO_ROOT

# Apt packages needed to build the app on Linux (kept in sync with .github/workflows/ci.yml
# and scripts/docker/linux.Dockerfile).
LINUX_DOCKER_IMAGE="dyktando-x-linux-build:latest"

# Builds (or reuses) the Linux build image. Docker Desktop on Apple Silicon runs it through
# amd64 emulation, so everything inside is noticeably slower than a native build.
ensure_linux_image() {
  need docker
  docker info >/dev/null 2>&1 || die "Docker is not running"
  log "Building Docker image $LINUX_DOCKER_IMAGE (cached after the first run)"
  docker build --platform linux/amd64 -t "$LINUX_DOCKER_IMAGE" \
    -f "$REPO_ROOT/scripts/docker/linux.Dockerfile" "$REPO_ROOT/scripts/docker" >&2
}

# Runs a command inside the Linux build container with the repository mounted at /src.
# node_modules, the cargo registry and the cargo target dir live in named volumes so the
# host checkout (macOS binaries) is never mixed with Linux ones.
run_in_linux_container() {
  local -a env_args=()
  local var
  for var in TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD; do
    [[ -n "${!var:-}" ]] && env_args+=(-e "$var")
  done
  docker run --rm --platform linux/amd64 \
    -v "$REPO_ROOT:/src" \
    -v dyktando-x-node-modules:/src/node_modules \
    -v dyktando-x-cargo-registry:/root/.cargo/registry \
    -v dyktando-x-cargo-git:/root/.cargo/git \
    -v dyktando-x-linux-target:/target \
    -v dyktando-x-tauri-cache:/root/.cache/tauri \
    -e CARGO_TARGET_DIR=/target \
    -e APPIMAGE_EXTRACT_AND_RUN=1 \
    -e CI=true \
    "${env_args[@]}" \
    -w /src \
    "$LINUX_DOCKER_IMAGE" bash -euo pipefail -c "$1"
}
