#!/usr/bin/env bash
# Runs the same checks as .github/workflows/ci.yml, locally:
#   npm ci + npm run build (TypeScript + Vite), cargo test --lib --locked,
#   npx tauri build --debug --no-bundle.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

usage() {
  cat <<'USAGE'
Usage: scripts/ci.sh [--linux] [--help]

Runs the CI checks (frontend build, Rust unit tests, debug app build) on the host platform.

  --linux   Run the checks inside the Linux x86_64 Docker image (scripts/docker/linux.Dockerfile)
            instead of on the host. Slow on Apple Silicon (amd64 emulation).
  --help    Show this help.
USAGE
}

LINUX=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --linux) LINUX=1 ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "unknown argument: $1" ;;
  esac
  shift
done

CHECKS='
  echo "==> Frontend (TypeScript + Vite)"
  npm ci
  npm run build
  echo "==> Rust tests"
  (cd src-tauri && cargo test --lib --locked)
  echo "==> Build app (debug, no bundles)"
  npx tauri build --debug --no-bundle
'

if [[ $LINUX -eq 1 ]]; then
  ensure_linux_image
  log "Running CI checks in Docker (linux/amd64)"
  run_in_linux_container "$CHECKS"
else
  need node npm cargo
  cd "$REPO_ROOT"
  bash -euo pipefail -c "$CHECKS"
fi
log "CI checks passed"
