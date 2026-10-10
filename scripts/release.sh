#!/usr/bin/env bash
# Builds, signs and publishes a Dyktando X release from this machine (replaces the GitHub
# Actions release workflow). See RELEASING.md.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
source "$(dirname "${BASH_SOURCE[0]}")/bump-version.sh"   # check_versions_agree

usage() {
  cat <<'USAGE'
Usage: scripts/release.sh [options] [--mac] [--linux] [--windows]

Builds the release for the selected platforms (default: all three), verifies signatures,
writes the updater manifest (latest.json) and, unless --dry-run, tags vX.Y.Z, creates the
GitHub release, uploads every asset and publishes it as the latest release.

Platforms:
  --mac          macOS Apple Silicon (.dmg + updater .app.tar.gz), Developer ID signed,
                 notarized and stapled.
  --linux        Linux x86_64 (.deb, .rpm, .AppImage), built in Docker (linux/amd64).
  --windows      Windows x64 NSIS installer, cross-compiled with cargo-xwin.
                 (MSI needs WiX, which only runs on Windows, so it is not built locally.)

Options:
  --windows-gh   Build Windows on GitHub Actions instead (release.yml, workflow_dispatch)
                 and attach its installer to the release. Not possible with --dry-run.
  --dry-run      Build, sign, notarize and verify everything into out/release/vX.Y.Z/,
                 but do not tag, push or touch GitHub. Branch/tag checks become warnings.
  --draft        Leave the GitHub release as a draft (the in-app updater ignores drafts).
  --skip-ci      Do not run scripts/ci.sh first.
  --keep-build   Keep the per-platform cargo build directories (default: removed after
                 the artifacts are copied, to save disk space).
  --notes TEXT   Release notes for latest.json (shown by the in-app updater). Default: empty.
  -h, --help     Show this help.

Environment: RELEASE_MIN_FREE_GB (default 20) — free disk required by the preflight.

Environment / keys (see RELEASING.md):
  ~/.config/local-release/apple.env          APPLE_ID, APPLE_TEAM_ID, APPLE_SIGNING_IDENTITY (SHA-1)
  keychain item local-release-apple-password  app-specific password (read at runtime, never stored)
  ~/.config/local-release/tauri-updater.env  DYKTANDO_X_UPDATER_KEY (+ <key>.password)
USAGE
}

DRY_RUN=0 DRAFT=0 SKIP_CI=0 KEEP_BUILD=0 WINDOWS_GH=0 NOTES=""
DO_MAC=0 DO_LINUX=0 DO_WINDOWS=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mac) DO_MAC=1 ;;
    --linux) DO_LINUX=1 ;;
    --windows) DO_WINDOWS=1 ;;
    --windows-gh) DO_WINDOWS=1; WINDOWS_GH=1 ;;
    --dry-run) DRY_RUN=1 ;;
    --draft) DRAFT=1 ;;
    --skip-ci) SKIP_CI=1 ;;
    --keep-build) KEEP_BUILD=1 ;;
    --notes) shift; NOTES="${1:?--notes needs a value}" ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "unknown argument: $1" ;;
  esac
  shift
done
if [[ $((DO_MAC + DO_LINUX + DO_WINDOWS)) -eq 0 ]]; then DO_MAC=1 DO_LINUX=1 DO_WINDOWS=1; fi
[[ $WINDOWS_GH -eq 1 && $DRY_RUN -eq 1 ]] && die "--windows-gh needs a real release (no --dry-run)"

cd "$REPO_ROOT"
TAURI_DIR="$REPO_ROOT/src-tauri"
PRODUCT="Dyktando X"
# GitHub replaces spaces in asset names with dots; name the files that way up front so the
# URLs in latest.json are predictable.
ASSET_PREFIX="${PRODUCT// /.}"
MIN_FREE_GB="${RELEASE_MIN_FREE_GB:-20}"

# ---------------------------------------------------------------------------- preflight

preflight() {
  log "Preflight"
  need git node npm npx cargo jq gh
  [[ $DO_MAC -eq 1 ]] && need xcrun codesign spctl security hdiutil
  [[ $DO_LINUX -eq 1 ]] && need docker
  [[ $DO_WINDOWS -eq 1 && $WINDOWS_GH -eq 0 ]] && need cargo-xwin makensis

  VERSION="$(check_versions_agree)"
  TAG="v$VERSION"
  OUT="$REPO_ROOT/out/release/$TAG"
  REPO_SLUG="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
  log "Version $VERSION ($TAG), repository $REPO_SLUG"

  local problems=()
  [[ -z "$(git status --porcelain)" ]] || problems+=("working tree is not clean")
  [[ "$(git rev-parse --abbrev-ref HEAD)" == "main" ]] || problems+=("not on main")
  git fetch -q origin main --tags
  [[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || problems+=("HEAD is not equal to origin/main")

  RESUME=0
  if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || git ls-remote --exit-code --tags origin "$TAG" >/dev/null 2>&1; then
    local is_draft
    is_draft="$(gh release view "$TAG" --json isDraft -q .isDraft 2>/dev/null || echo missing)"
    if [[ "$is_draft" == "true" ]]; then
      RESUME=1
      warn "tag $TAG exists with a draft release: re-running uploads into that draft"
      [[ "$(git rev-parse "$TAG^{commit}" 2>/dev/null)" == "$(git rev-parse HEAD)" ]] \
        || problems+=("tag $TAG points to a different commit than HEAD")
    else
      problems+=("tag $TAG already exists (release: $is_draft) — bump the version first")
    fi
  fi

  if [[ ${#problems[@]} -gt 0 ]]; then
    local p
    if [[ $DRY_RUN -eq 1 ]]; then
      for p in "${problems[@]}"; do warn "$p (ignored for --dry-run)"; done
    else
      for p in "${problems[@]}"; do printf '  - %s\n' "$p" >&2; done
      die "preflight failed"
    fi
  fi

  # Updater signing key (minisign). Content goes to the environment only.
  local updater_env="$HOME/.config/local-release/tauri-updater.env"
  [[ -f "$updater_env" ]] || die "missing $updater_env"
  # shellcheck disable=SC1090
  source "$updater_env"
  [[ -f "${DYKTANDO_X_UPDATER_KEY:-}" ]] || die "DYKTANDO_X_UPDATER_KEY does not point to a key file"
  TAURI_SIGNING_PRIVATE_KEY="$(cat "$DYKTANDO_X_UPDATER_KEY")"
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$DYKTANDO_X_UPDATER_KEY.password" 2>/dev/null || true)"
  export TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD
  UPDATER_PUBKEY="$(jq -r .plugins.updater.pubkey "$TAURI_DIR/tauri.conf.json")"

  if [[ $DO_MAC -eq 1 ]]; then
    local apple_env="$HOME/.config/local-release/apple.env"
    [[ -f "$apple_env" ]] || die "missing $apple_env"
    # shellcheck disable=SC1090
    source "$apple_env"
    : "${APPLE_ID:?}" "${APPLE_TEAM_ID:?}" "${APPLE_SIGNING_IDENTITY:?}" "${NOTARY_PROFILE:?}"
    grep -q "$APPLE_SIGNING_IDENTITY" <<<"$(security find-identity -v -p codesigning)" \
      || die "signing identity $APPLE_SIGNING_IDENTITY not found in the keychain"
    security find-generic-password -s local-release-apple-password >/dev/null 2>&1 \
      || die "keychain item local-release-apple-password not found"
    grep -qx aarch64-apple-darwin <<<"$(rustup target list --installed)" || die "rustup target aarch64-apple-darwin missing"
  fi
  if [[ $DO_WINDOWS -eq 1 && $WINDOWS_GH -eq 0 ]]; then
    grep -qx x86_64-pc-windows-msvc <<<"$(rustup target list --installed)" || die "rustup target x86_64-pc-windows-msvc missing"
    [[ -x /opt/homebrew/opt/llvm/bin/clang-cl ]] || command -v clang-cl >/dev/null || die "clang-cl not found (brew install llvm)"
  fi
  if [[ $DO_LINUX -eq 1 ]]; then
    docker info >/dev/null 2>&1 || die "Docker is not running"
  fi

  local free_gb
  free_gb="$(df -g "$REPO_ROOT" | awk 'NR==2 {print $4}')"
  [[ "$free_gb" -ge $MIN_FREE_GB ]] || die "only ${free_gb} GB free on disk, need at least ${MIN_FREE_GB} GB"
  log "Free disk: ${free_gb} GB"

  mkdir -p "$OUT"
}

# Copies a file into $OUT, replacing spaces in its name with dots (or as $2 when given).
collect() {
  local src="$1" name="${2:-}"
  [[ -n "$name" ]] || name="$(basename "$src")"
  name="${name// /.}"
  cp -R "$src" "$OUT/$name"
  printf '%s\n' "$OUT/$name"
}

verify_sig() {
  node "$REPO_ROOT/scripts/lib/verify-minisign.mjs" "$1" "$1.sig" "$UPDATER_PUBKEY" >&2
}

# ---------------------------------------------------------------------------- macOS

build_mac() {
  local target=aarch64-apple-darwin
  local bundle="$TAURI_DIR/target/$target/release/bundle"
  log "macOS: building, signing and notarizing ($target)"
  rm -rf "$bundle/macos" "$bundle/dmg"
  # APPLE_PASSWORD only lives in this process's environment.
  APPLE_PASSWORD="$(security find-generic-password -s local-release-apple-password -w)" \
  APPLE_SIGNING_IDENTITY="$APPLE_SIGNING_IDENTITY" APPLE_ID="$APPLE_ID" APPLE_TEAM_ID="$APPLE_TEAM_ID" \
    npx tauri build --target "$target" --bundles app,dmg

  local app="$bundle/macos/$PRODUCT.app"
  local dmg
  dmg="$(ls "$bundle/dmg/"*.dmg)"
  verify_app "$app"

  # Tauri notarizes the .app; the .dmg is signed and notarized here.
  if ! codesign -dv "$dmg" >/dev/null 2>&1; then
    log "macOS: signing the dmg"
    codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$dmg"
  fi
  log "macOS: notarizing the dmg (this waits for Apple)"
  xcrun notarytool submit "$dmg" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
  codesign --verify --strict --verbose=2 "$dmg"
  spctl -a -vv -t open --context context:primary-signature "$dmg"

  local tarball="$bundle/macos/$PRODUCT.app.tar.gz"
  [[ -f "$tarball" && -f "$tarball.sig" ]] || die "updater archive $tarball(.sig) missing"
  # The updater archive must contain the stapled app.
  local check_dir
  check_dir="$(mktemp -d)"
  tar -xzf "$tarball" -C "$check_dir"
  verify_app "$check_dir/$PRODUCT.app"
  rm -rf "$check_dir"

  local f
  f="$(collect "$dmg")"; log "  $f"
  # Same name tauri-action used: Dyktando.X_aarch64.app.tar.gz.
  local tar_name="${ASSET_PREFIX}_aarch64.app.tar.gz"
  f="$(collect "$tarball" "$tar_name")"; collect "$tarball.sig" "$tar_name.sig" >/dev/null; verify_sig "$f"
  MAC_TARBALL="$(basename "$f")"
  [[ $KEEP_BUILD -eq 1 ]] || rm -rf "$TAURI_DIR/target/$target"
}

verify_app() {
  local app="$1"
  log "macOS: verifying $(basename "$app")"
  codesign --verify --deep --strict --verbose=2 "$app"
  spctl -a -vv -t exec "$app"
  xcrun stapler validate "$app"
  local details
  details="$(codesign -dv "$app" 2>&1)"
  grep -qx "TeamIdentifier=$APPLE_TEAM_ID" <<<"$details" || die "$app is not signed by team $APPLE_TEAM_ID"
}

# ---------------------------------------------------------------------------- Linux

build_linux() {
  log "Linux: building deb/rpm/AppImage in Docker (linux/amd64, slow under emulation)"
  ensure_linux_image
  local stage="out/.stage-linux"
  rm -rf "$REPO_ROOT/$stage"
  # The AppImage tools are AppImages themselves. Their magic bytes (ELF offset 8) make
  # Docker's amd64 emulation refuse to execute them ("Exec format error"), so the appimage
  # plugin is fetched into Tauri's tool cache up front and the magic bytes are zeroed.
  run_in_linux_container "
    mkdir -p /root/.cache/tauri
    plugin=/root/.cache/tauri/linuxdeploy-plugin-appimage.AppImage
    [ -f \$plugin ] || curl -fsSL -o \$plugin \
      https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-x86_64.AppImage
    for f in /root/.cache/tauri/*.AppImage; do
      chmod +x \$f; dd if=/dev/zero of=\$f bs=1 count=3 seek=8 conv=notrunc 2>/dev/null
    done
    npm ci
    npx tauri build --bundles deb,rpm,appimage
    mkdir -p $stage
    cp /target/release/bundle/deb/*.deb* /target/release/bundle/rpm/*.rpm* \
       /target/release/bundle/appimage/*.AppImage* $stage/
    rm -rf /target/release/bundle
  "
  local f
  for f in "$REPO_ROOT/$stage"/*; do
    [[ "$f" == *.sig ]] && continue
    [[ -f "$f.sig" ]] || die "missing signature for $f"
    local copied
    copied="$(collect "$f")"; collect "$f.sig" >/dev/null
    verify_sig "$copied"
    case "$copied" in
      *.AppImage) LINUX_APPIMAGE="$(basename "$copied")" ;;
      *.deb) LINUX_DEB="$(basename "$copied")" ;;
      *.rpm) LINUX_RPM="$(basename "$copied")" ;;
    esac
  done
  rm -rf "${REPO_ROOT:?}/$stage"
  [[ -n "${LINUX_APPIMAGE:-}" && -n "${LINUX_DEB:-}" && -n "${LINUX_RPM:-}" ]] || die "Linux bundles incomplete"
}

# ---------------------------------------------------------------------------- Windows

build_windows() {
  local target=x86_64-pc-windows-msvc
  local bundle="$TAURI_DIR/target/$target/release/bundle"
  log "Windows: cross-compiling with cargo-xwin ($target, NSIS)"
  # cargo-xwin needs clang-cl/llvm-lib (Homebrew llvm) and lld-link; Homebrew llvm ships no
  # lld-link, so fall back to Rust's bundled rust-lld, which acts as lld-link under that name.
  local shim
  shim="$(mktemp -d)"
  if ! command -v lld-link >/dev/null; then
    ln -s "$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | awk '/^host/ {print $2}')/bin/rust-lld" "$shim/lld-link"
  fi
  # whisper-rs-sys picks "link ggml-blas" from the HOST OS (macOS) instead of the target, so the
  # Windows link would look for a BLAS library that is never built. An empty one satisfies it.
  local llvm=/opt/homebrew/opt/llvm/bin
  echo 'void dyktando_x_ggml_blas_stub(void) {}' > "$shim/stub.c"
  (cd "$shim" && "$llvm/clang-cl" --target=x86_64-pc-windows-msvc /nologo /c stub.c /Fostub.obj \
    && "$llvm/llvm-lib" /nologo /out:ggml-blas.lib stub.obj)
  rm -rf "$bundle/nsis"
  PATH="$shim:$llvm:$PATH" \
  CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-L native=$shim" \
    npx tauri build --runner cargo-xwin --target "$target" --bundles nsis
  rm -rf "$shim"

  local exe
  exe="$(ls "$bundle/nsis/"*-setup.exe)"
  [[ -f "$exe.sig" ]] || die "missing signature for $exe"
  local f
  f="$(collect "$exe")"; collect "$exe.sig" >/dev/null; verify_sig "$f"
  WINDOWS_EXE="$(basename "$f")"
  [[ $KEEP_BUILD -eq 1 ]] || rm -rf "$TAURI_DIR/target/$target"
}

# Fallback: builds the Windows installer on GitHub Actions (release.yml) and downloads it
# from the draft release so latest.json can include it.
build_windows_gh() {
  log "Windows: dispatching release.yml on GitHub Actions for $TAG"
  local before run_id
  before="$(gh run list --workflow release.yml -L 1 --json databaseId -q '.[0].databaseId // 0')"
  gh workflow run release.yml --ref main -f tag="$TAG" -f platforms=windows
  for _ in $(seq 1 30); do
    run_id="$(gh run list --workflow release.yml -L 1 --json databaseId -q '.[0].databaseId // 0')"
    [[ "$run_id" != "$before" ]] && break
    sleep 5
  done
  [[ "$run_id" != "$before" ]] || die "workflow run did not start"
  gh run watch "$run_id" --exit-status
  gh release download "$TAG" -D "$OUT" --clobber -p '*-setup.exe' -p '*-setup.exe.sig'
  WINDOWS_EXE="$(cd "$OUT" && ls *-setup.exe)"
  verify_sig "$OUT/$WINDOWS_EXE"
}

# ---------------------------------------------------------------------------- latest.json

write_latest_json() {
  local base="https://github.com/$REPO_SLUG/releases/download/$TAG"
  local platforms='{}'
  add() { # key file
    platforms="$(jq --arg k "$1" --arg url "$base/$2" --rawfile sig "$OUT/$2.sig" \
      '. + {($k): {signature: $sig, url: $url}}' <<<"$platforms")"
  }
  # Dry runs are often done one platform at a time: include what earlier dry runs of this
  # version left in $OUT so the combined manifest can be checked.
  if [[ $DRY_RUN -eq 1 ]]; then
    pick() { (cd "$OUT" && ls $1 2>/dev/null | grep -v '\.sig$' | head -1) || true; }
    : "${MAC_TARBALL:=$(pick '*_aarch64.app.tar.gz')}"
    : "${LINUX_APPIMAGE:=$(pick '*.AppImage')}" "${LINUX_DEB:=$(pick '*.deb')}" "${LINUX_RPM:=$(pick '*.rpm')}"
    : "${WINDOWS_EXE:=$(pick '*-setup.exe')}"
  fi
  if [[ -n "${MAC_TARBALL:-}" ]]; then add darwin-aarch64 "$MAC_TARBALL"; add darwin-aarch64-app "$MAC_TARBALL"; fi
  if [[ -n "${LINUX_APPIMAGE:-}" ]]; then
    add linux-x86_64 "$LINUX_APPIMAGE"; add linux-x86_64-appimage "$LINUX_APPIMAGE"
    add linux-x86_64-deb "$LINUX_DEB"; add linux-x86_64-rpm "$LINUX_RPM"
  fi
  if [[ -n "${WINDOWS_EXE:-}" ]]; then add windows-x86_64 "$WINDOWS_EXE"; add windows-x86_64-nsis "$WINDOWS_EXE"; fi

  # A partial run (e.g. only --mac) into an existing draft keeps the other platforms.
  if [[ $RESUME -eq 1 && $DRY_RUN -eq 0 ]]; then
    local existing
    existing="$(gh release download "$TAG" -p latest.json -O - 2>/dev/null || echo '{}')"
    platforms="$(jq -n --argjson old "$(jq '.platforms // {}' <<<"$existing")" --argjson new "$platforms" '$old + $new')"
  fi

  jq -n --arg version "$VERSION" --arg notes "$NOTES" \
    --arg pub_date "$(date -u +%Y-%m-%dT%H:%M:%S.000Z)" --argjson platforms "$platforms" \
    '{version: $version, notes: $notes, pub_date: $pub_date, platforms: $platforms}' > "$OUT/latest.json"
  jq -e '.platforms | length > 0' "$OUT/latest.json" >/dev/null || die "latest.json has no platforms"
  log "latest.json: $(jq -r '.platforms | keys | join(", ")' "$OUT/latest.json")"
}

# ---------------------------------------------------------------------------- publish

publish() {
  if [[ $RESUME -eq 0 ]]; then
    log "Tagging $TAG and pushing the tag"
    git tag -a "$TAG" -m "$PRODUCT $TAG"
    git push origin "$TAG"
    gh release create "$TAG" --draft --title "$PRODUCT $TAG" --generate-notes --verify-tag
  fi
  log "Uploading assets"
  local -a assets=()
  local f
  for f in "$OUT"/*; do [[ -f "$f" ]] && assets+=("$f"); done
  gh release upload "$TAG" "${assets[@]}" --clobber
  if [[ $DRAFT -eq 1 ]]; then
    log "Release left as a draft: $(gh release view "$TAG" --json url -q .url)"
  else
    gh release edit "$TAG" --draft=false --latest
    log "Published: $(gh release view "$TAG" --json url -q .url)"
  fi
}

# ---------------------------------------------------------------------------- main

preflight
if [[ $SKIP_CI -eq 0 ]]; then
  "$REPO_ROOT/scripts/ci.sh"
  # The CI debug build is not needed for the release; free its disk space.
  [[ $KEEP_BUILD -eq 1 ]] || rm -rf "$TAURI_DIR/target/debug"
fi

[[ $DO_MAC -eq 1 ]] && build_mac
[[ $DO_LINUX -eq 1 ]] && build_linux
if [[ $DO_WINDOWS -eq 1 && $WINDOWS_GH -eq 0 ]]; then build_windows; fi

if [[ $DRY_RUN -eq 1 ]]; then
  write_latest_json
  log "Dry run complete — artifacts in ${OUT#"$REPO_ROOT"/}:"
  ls -lh "$OUT" >&2
  exit 0
fi

if [[ $WINDOWS_GH -eq 1 ]]; then
  # The workflow uploads into the draft release, so it has to exist first.
  if [[ $RESUME -eq 0 ]]; then
    DRAFT_SAVED=$DRAFT; DRAFT=1; publish; DRAFT=$DRAFT_SAVED; RESUME=1
  fi
  build_windows_gh
fi
write_latest_json
publish
