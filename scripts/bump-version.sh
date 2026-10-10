#!/usr/bin/env bash
# Writes a new version into every version file, verifies they agree and commits
# "chore: release X.Y.Z". Does not tag or push (scripts/release.sh tags).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

usage() {
  cat <<'USAGE'
Usage: scripts/bump-version.sh X.Y.Z [--no-commit]

Updates the version in:
  package.json, package-lock.json, src-tauri/tauri.conf.json,
  src-tauri/Cargo.toml, src-tauri/Cargo.lock
then commits "chore: release X.Y.Z" (unless --no-commit).
USAGE
}

# Prints the version from each version file, one "file version" pair per line.
read_versions() {
  cd "$REPO_ROOT"
  printf 'package.json %s\n' "$(node -p 'require("./package.json").version')"
  printf 'package-lock.json %s\n' "$(node -p 'const l=require("./package-lock.json"); l.version + (l.packages[""].version===l.version ? "" : "!=" + l.packages[""].version)')"
  printf 'src-tauri/tauri.conf.json %s\n' "$(node -p 'require("./src-tauri/tauri.conf.json").version')"
  printf 'src-tauri/Cargo.toml %s\n' "$(sed -n '/^\[package\]/,/^\[/s/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml)"
  printf 'src-tauri/Cargo.lock %s\n' "$(awk '/^name = "dyktando-x"$/{getline; gsub(/version = |"/,""); print}' src-tauri/Cargo.lock)"
}

# Fails unless every version file holds the same version; prints that version.
check_versions_agree() {
  local versions unique
  versions="$(read_versions)"
  unique="$(awk '{print $2}' <<<"$versions" | sort -u)"
  if [[ "$(wc -l <<<"$unique" | tr -d ' ')" != 1 ]]; then
    printf '%s\n' "$versions" >&2
    die "version files disagree"
  fi
  printf '%s\n' "$unique"
}

# Allow `source scripts/bump-version.sh` from release.sh to reuse the checks.
[[ "${BASH_SOURCE[0]}" != "$0" ]] && return 0

COMMIT=1
VERSION=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --no-commit) COMMIT=0 ;;
    -*) usage; die "unknown option: $1" ;;
    *) [[ -z "$VERSION" ]] || die "only one version expected"; VERSION="$1" ;;
  esac
  shift
done
[[ -n "$VERSION" ]] || { usage; exit 1; }
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must look like X.Y.Z, got: $VERSION"
need node git

cd "$REPO_ROOT"
if [[ $COMMIT -eq 1 && -n "$(git status --porcelain)" ]]; then
  die "working tree is not clean; commit or stash first"
fi

log "Setting version $VERSION"
V="$VERSION" node - <<'JS'
const fs = require("fs");
const v = process.env.V;
const editJson = (file, fn) => {
  const data = JSON.parse(fs.readFileSync(file, "utf8"));
  fn(data);
  fs.writeFileSync(file, JSON.stringify(data, null, 2) + "\n");
};
editJson("package.json", (d) => { d.version = v; });
editJson("package-lock.json", (d) => { d.version = v; d.packages[""].version = v; });
editJson("src-tauri/tauri.conf.json", (d) => { d.version = v; });

const cargoToml = "src-tauri/Cargo.toml";
let toml = fs.readFileSync(cargoToml, "utf8");
toml = toml.replace(/(\[package\][^\[]*?\nversion = ")[^"]*(")/, `$1${v}$2`);
fs.writeFileSync(cargoToml, toml);

const cargoLock = "src-tauri/Cargo.lock";
let lock = fs.readFileSync(cargoLock, "utf8");
lock = lock.replace(/(name = "dyktando-x"\nversion = ")[^"]*(")/, `$1${v}$2`);
fs.writeFileSync(cargoLock, lock);
JS

AGREED="$(check_versions_agree)"
[[ "$AGREED" == "$VERSION" ]] || die "expected $VERSION in all files, got $AGREED"
log "All version files at $VERSION"

if [[ $COMMIT -eq 1 ]]; then
  git add package.json package-lock.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
  git commit -q -m "chore: release $VERSION"
  log "Committed: chore: release $VERSION (not tagged, not pushed)"
fi
