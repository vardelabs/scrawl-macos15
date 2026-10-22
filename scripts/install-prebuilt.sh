#!/usr/bin/env bash
# Installs a prebuilt Scrawl fork release (the zip that package-prebuilt.sh makes, where this file
# is install.sh beside Scrawl.app), with Parakeet transcription and the dictation formatter, on an
# Apple silicon Mac running macOS 15 or later. Needs Homebrew, not Xcode.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="${HOME}/Applications/Scrawl.app"
SHARE_DIR="${HOME}/.local/share/scrawl"
CONFIG="${HOME}/.config/scrawl/config.json"
PARAKEET_PY="${HOME}/.local/share/uv/tools/parakeet-mlx/bin/python"
FORMAT_PY="${HOME}/.local/share/uv/tools/mlx-lm/bin/python"

[[ "$(uname -m)" == "arm64" ]] || { echo "Scrawl needs an Apple silicon Mac." >&2; exit 1; }
major="$(sw_vers -productVersion | cut -d. -f1)"
((major >= 15)) || { echo "Scrawl needs macOS 15 or later." >&2; exit 1; }
command -v brew >/dev/null || { echo "Install Homebrew first: https://brew.sh" >&2; exit 1; }

echo "Installing Scrawl to ${APP_DIR}"
pkill -f "Scrawl.app/Contents/MacOS/scrawl" 2>/dev/null || true
mkdir -p "${HOME}/Applications"
rm -rf "${APP_DIR}"
/usr/bin/ditto "${HERE}/Scrawl.app" "${APP_DIR}"
xattr -dr com.apple.quarantine "${APP_DIR}" 2>/dev/null || true

# The bundle arrives signed by the build Mac's self-signed identity, which this Mac does not
# trust. Re-signing with a local identity made here keeps permission grants across updates.
"${HERE}/create-local-signing.sh"
IDENTITY="Scrawl Local Signing"
codesign --force --sign "${IDENTITY}" --identifier "dev.kitlangton.scrawl.helper" "${APP_DIR}/Contents/MacOS/scrawl-helper"
codesign --force --sign "${IDENTITY}" --identifier "dev.kitlangton.scrawl" "${APP_DIR}"
codesign --verify --deep --strict "${APP_DIR}"

echo "Installing Parakeet (uv, ffmpeg, parakeet-mlx)"
brew list uv >/dev/null 2>&1 || brew install uv
brew list ffmpeg >/dev/null 2>&1 || brew install ffmpeg
uv tool list 2>/dev/null | grep -q '^parakeet-mlx ' || uv tool install parakeet-mlx
mkdir -p "${SHARE_DIR}"
cp -f "${HERE}/scrawl-parakeet.py" "${SHARE_DIR}/scrawl-parakeet.py"

# The first run downloads the model (about 1.2 GB), which would outlast Scrawl's timeout.
echo "Downloading the Parakeet model (about 1.2 GB) and checking it runs"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
ffmpeg -loglevel error -f lavfi -i anullsrc=r=16000:cl=mono -t 1 "${WORK}/silence.wav"
"${PARAKEET_PY}" "${SHARE_DIR}/scrawl-parakeet.py" "${WORK}/silence.wav" >/dev/null

echo "Installing the dictation formatter (mlx-lm)"
uv tool list 2>/dev/null | grep -q '^mlx-lm ' || uv tool install mlx-lm
cp -f "${HERE}/scrawl-format.py" "${SHARE_DIR}/scrawl-format.py"

# Its first run downloads the model too (about 1.8 GB); with nothing on stdin it loads and exits.
echo "Downloading the formatter model (about 1.8 GB) and checking it runs"
SCRAWL_PRESTART=1 "${FORMAT_PY}" "${SHARE_DIR}/scrawl-format.py" </dev/null >/dev/null

echo "Pointing Scrawl at Parakeet and the formatter in ${CONFIG}"
mkdir -p "$(dirname "${CONFIG}")"
[[ -f "${CONFIG}" ]] && cp -f "${CONFIG}" "${CONFIG}.bak"
"${PARAKEET_PY}" - "${CONFIG}" "${PARAKEET_PY}" "${SHARE_DIR}/scrawl-parakeet.py" "${FORMAT_PY}" "${SHARE_DIR}/scrawl-format.py" <<'PY'
import json, os, sys
path, python, script, format_python, format_script = sys.argv[1:6]
config = {}
if os.path.exists(path):
    with open(path) as f:
        config = json.load(f)
# An update runs this again, so a choice made in Settings since the first install stays.
for key, value in {
    "transcriber": "command",
    "transcribeCommand": [python, script, "{audio}"],
    "transcribePrestart": True,
    "formatCommand": [format_python, format_script],
}.items():
    config.setdefault(key, value)
with open(path, "w") as f:
    json.dump(config, f, indent=2)
    f.write("\n")
PY

open -a "${APP_DIR}"
echo "Done. Scrawl is in the menu bar. Grant the permissions its welcome window asks for."
