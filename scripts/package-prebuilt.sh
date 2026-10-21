#!/usr/bin/env bash
# Builds the prebuilt release zip of the committed HEAD in dist/prebuilt: a release Scrawl.app, with
# install.sh and the files it needs beside it, so a Mac without Xcode can install the fork.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${REPO_DIR}/dist/prebuilt"
NAME="Scrawl-Fork-$(git -C "${REPO_DIR}" rev-parse --short HEAD)"
PKG="${OUT_DIR}/${NAME}"
APP="${PKG}/Scrawl.app"

# The release is public, and the Metal compiler writes each shader's absolute source path into the
# binary, out of reach of rustc's path remapping. Building from neutral paths keeps the build Mac's
# home folder out of it. The Cargo home is a real directory, because Cargo resolves a symlink, and
# its crates download once and are reused.
BUILD_DIR="/tmp/scrawl-prebuilt"
NEUTRAL_CARGO_HOME="/tmp/scrawl-prebuilt-cargo"
[[ -L "${NEUTRAL_CARGO_HOME}" ]] && rm -f "${NEUTRAL_CARGO_HOME}"
mkdir -p "${NEUTRAL_CARGO_HOME}"
git -C "${REPO_DIR}" worktree remove --force "${BUILD_DIR}" 2>/dev/null || rm -rf "${BUILD_DIR}"
git -C "${REPO_DIR}" worktree add --detach "${BUILD_DIR}" HEAD
trap 'git -C "${REPO_DIR}" worktree remove --force "${BUILD_DIR}"' EXIT

rm -rf "${OUT_DIR}"
mkdir -p "${PKG}"

export CARGO_HOME="${NEUTRAL_CARGO_HOME}"
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${HOME}=/home"
# Sparkle reads Kit's update feed, so it would replace the fork with Kit's build.
SCRAWL_BUILD_COMMIT="$(git -C "${REPO_DIR}" rev-parse HEAD)" SCRAWL_NO_SPARKLE=1 SCRAWL_PROFILE=release SCRAWL_APP_DIR="${APP}" SCRAWL_NO_LAUNCH=1 \
  "${BUILD_DIR}/scripts/install-app.sh"

leaks="$(strings "${APP}/Contents/MacOS/scrawl" "${APP}/Contents/MacOS/scrawl-helper" | grep -cF -- "${USER}" || true)"
if ((leaks > 0)); then
  echo "The binaries still contain the build user's name ${leaks} times; not packaging." >&2
  exit 1
fi

cp -f "${REPO_DIR}/scripts/install-prebuilt.sh" "${PKG}/install.sh"
cp -f "${REPO_DIR}/scripts/create-local-signing.sh" "${REPO_DIR}/scripts/scrawl-parakeet.py" "${PKG}/"
chmod +x "${PKG}/install.sh" "${PKG}/create-local-signing.sh" "${PKG}/scrawl-parakeet.py"

/usr/bin/ditto -c -k --norsrc --noextattr --keepParent "${PKG}" "${OUT_DIR}/${NAME}.zip"
(cd "${OUT_DIR}" && shasum -a 256 "${NAME}.zip" >"${NAME}.zip.sha256")
echo "${OUT_DIR}/${NAME}.zip"
