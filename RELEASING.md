# Releasing Dyktando X

Releases are built on a Mac with one command. GitHub Actions (`ci.yml`, `release.yml`) are kept
only as a manual fallback (`workflow_dispatch`), because hosted macOS and Windows minutes are
expensive.

```bash
scripts/bump-version.sh 0.3.9      # writes the version everywhere, commits "chore: release 0.3.9"
git push origin main               # release.sh requires main == origin/main
scripts/release.sh                 # macOS + Linux + Windows, tag, GitHub release, publish
```

Useful variants:

```bash
scripts/release.sh --dry-run                 # build, sign, notarize, verify; touch nothing on GitHub
scripts/release.sh --dry-run --mac --skip-ci # one platform only
scripts/release.sh --draft                   # leave the release as a draft
scripts/release.sh --windows-gh              # Windows on GitHub Actions instead of cargo-xwin
scripts/ci.sh                                # the CI checks on this machine
scripts/ci.sh --linux                        # the CI checks in the Linux Docker image
```

Artifacts land in `out/release/vX.Y.Z/` (gitignored).

## One-time machine setup

- Xcode (command line tools), Rust 1.96 via rustup with the targets
  `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`, Node 22+ with npm, `gh` (logged in),
  `jq`, Docker Desktop (Linux builds run as `linux/amd64`, give the VM ~80 GB of disk).
- Windows cross-compilation: `cargo install cargo-xwin`, `brew install llvm nsis`.
  `lld-link` comes from `brew install lld`; when it is missing, `release.sh` uses Rust's bundled
  `rust-lld` under that name. The first build downloads the MSVC CRT and Windows SDK
  (~2 GB, cached in `~/Library/Caches/cargo-xwin`), which means accepting the Microsoft license.
- Optional pre-push hook that runs `scripts/ci.sh`: `git config core.hooksPath .githooks`.

### Keys and where they live

| What | Where |
|---|---|
| Developer ID Application certificate | login keychain; signed by SHA-1 `APPLE_SIGNING_IDENTITY` (two certificates share the name, so never sign by name) |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_SIGNING_IDENTITY`, `NOTARY_PROFILE` | `~/.config/local-release/apple.env` |
| App-specific Apple ID password | keychain item `local-release-apple-password`; `release.sh` reads it into `APPLE_PASSWORD` for the Tauri build process only, it is never written to disk |
| notarytool credentials | keychain profile `local-release` (`xcrun notarytool store-credentials local-release`) |
| Updater signing key (minisign) | `~/.tauri/dyktando-x.key` (+ `.password`), path in `DYKTANDO_X_UPDATER_KEY` in `~/.config/local-release/tauri-updater.env`. The public key is `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`. **Losing this key means existing installs can no longer auto-update.** |

## What `scripts/release.sh` does

1. **Preflight:** clean working tree, on `main` and equal to `origin/main`, tag `vX.Y.Z` absent
   locally and on GitHub (or present with a *draft* release, in which case the run uploads into
   that draft again), all version files agree (`package.json`, `package-lock.json`,
   `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`), tools, keys and
   the signing identity are present, at least 20 GB of free disk. With `--dry-run` the git checks
   only warn.
2. **CI:** `scripts/ci.sh` (frontend build, `cargo test --lib --locked`, debug app build), unless
   `--skip-ci`.
3. **macOS** (`--target aarch64-apple-darwin`): `tauri build` signs with the Developer ID (hardened
   runtime, `Entitlements.plist`), notarizes and staples the `.app`, and creates the updater archive
   `Dyktando X.app.tar.gz` + `.sig`. The script then signs, notarizes (`notarytool --wait`) and
   staples the `.dmg`, and verifies the `.app`, the app inside the updater archive and the `.dmg`
   with `codesign --verify --deep --strict`, `spctl -a -vv`, `xcrun stapler validate` and a
   TeamIdentifier check.
4. **Linux x86_64:** builds `scripts/docker/linux.Dockerfile` (Ubuntu 24.04, the apt packages from
   CI, Node 22, Rust 1.96) and runs `tauri build --bundles deb,rpm,appimage` in it. The cargo
   registry and target dir are Docker volumes (`dyktando-x-cargo-registry`,
   `dyktando-x-linux-target`), so rebuilds are incremental. It runs under amd64 emulation on Apple
   Silicon and is slow (expect an hour or more for a cold build).
5. **Windows x64:** `tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis`.
   Only the NSIS installer is built: MSI needs WiX, which runs only on Windows, and the updater
   prefers NSIS anyway. The installer is not Authenticode-signed (same as before), so SmartScreen
   warns on first install.
6. **Updater:** every updater artifact's `.sig` is verified against the public key from
   `tauri.conf.json` (`scripts/lib/verify-minisign.mjs`), then `latest.json` is written with the
   same platform keys tauri-action used (`darwin-aarch64[-app]`, `linux-x86_64[-appimage|-deb|-rpm]`,
   `windows-x86_64[-nsis]`). Asset names use dots instead of spaces (`Dyktando.X_…`), exactly as
   GitHub renames them, so the URLs in `latest.json` match.
7. **Publish** (not with `--dry-run`): annotated tag `vX.Y.Z` pushed to GitHub, `gh release create`
   as a draft with generated notes, all assets uploaded (`--clobber`), then published as the latest
   release unless `--draft`. The in-app updater reads
   `releases/latest/download/latest.json`, so it ignores drafts.

Per-platform cargo build directories are deleted after their artifacts are copied (use
`--keep-build` to keep them). To reclaim the Linux build cache:
`docker volume rm dyktando-x-linux-target dyktando-x-cargo-registry dyktando-x-cargo-git dyktando-x-node-modules dyktando-x-tauri-cache`.

## Troubleshooting

- **Notarization fails:** `xcrun notarytool log <submission-id> --keychain-profile local-release`.
  "Invalid credentials" usually means the app-specific password in the keychain item expired.
- **`spctl` says "rejected" right after notarization:** the ticket may not be stapled yet; run
  `xcrun stapler staple` again on the `.dmg`, or check that the `.app` was notarized (Tauri prints
  "Notarizing" during the build; it only does so when `APPLE_ID`, `APPLE_PASSWORD` and
  `APPLE_TEAM_ID` are set).
- **cargo-xwin: `could not find native static library ggml-blas`:** whisper-rs-sys decides to link
  BLAS from the *host* OS (macOS), not the target. `release.sh` puts an empty `ggml-blas.lib` on the
  link path for the Windows target; if you build by hand, do the same.
- **`failed to run linuxdeploy` / `Exec format error` in the Linux build:** AppImage tools carry
  magic bytes in the ELF header that amd64 emulation refuses to run. `release.sh` zeroes them in the
  cached tools (`dyktando-x-tauri-cache` volume); delete that volume to start over.
- **Docker build is extremely slow / runs out of space:** make sure Docker Desktop uses Rosetta for
  amd64 emulation and has enough disk (Settings → Resources).
- **`tag vX.Y.Z already exists`:** bump the version (`scripts/bump-version.sh`); a published release is
  never overwritten. A draft release for the tag can be re-run.
- **A partial run** (e.g. `--mac` only) into an existing draft merges `latest.json` with the platforms
  already in that draft.

## Manual GitHub Actions fallback

Both workflows run only on demand:

- **CI:** Actions → CI → Run workflow (or `gh workflow run ci.yml --ref <branch>`).
- **Release:** create and push the tag first, then
  `gh workflow run release.yml -f tag=vX.Y.Z -f platforms=all` (or `windows`, `macos`, `linux`).
  It builds with tauri-action into a draft release for that tag and uploads `latest.json`; publish
  the draft manually afterwards. It needs the repository secrets `TAURI_SIGNING_PRIVATE_KEY`,
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` and, for a signed macOS build, `APPLE_CERTIFICATE`,
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`,
  `APPLE_TEAM_ID`.
