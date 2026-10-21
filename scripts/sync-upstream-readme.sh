#!/usr/bin/env bash
# Replays Kit's README.md changes between two upstream commits onto docs/guide.md, where his README
# lives in this fork, and keeps the fork's own README.md. Run it during every merge of upstream
# into macos-15, conflict or not: a README edit that merges cleanly would otherwise never reach the
# guide.
#
# Usage: scripts/sync-upstream-readme.sh OLD NEW
#   OLD: the last upstream commit already merged; NEW: the one being merged.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 OLD NEW" >&2
  exit 2
fi
OLD="$1"
NEW="$2"
cd "$(git rev-parse --show-toplevel)"

PATCH="$(mktemp)"
trap 'rm -f "${PATCH}"' EXIT
git diff "${OLD}" "${NEW}" -- README.md >"${PATCH}"
if [[ ! -s "${PATCH}" ]]; then
  echo "Kit's README did not change between ${OLD} and ${NEW}."
else
  # Only the file headers are renamed, so README.md inside the text stays as Kit wrote it.
  sed -i '' \
    -e 's#^diff --git a/README.md b/README.md$#diff --git a/docs/guide.md b/docs/guide.md#' \
    -e 's#^--- a/README.md$#--- a/docs/guide.md#' \
    -e 's#^+++ b/README.md$#+++ b/docs/guide.md#' \
    "${PATCH}"
  git apply -3 "${PATCH}"
  echo "Applied Kit's README changes to docs/guide.md."
  # The guide sits one level down, so a relative link Kit added needs its path adjusted by hand.
  if grep -E '^\+.*\]\((\./)?[A-Za-z0-9_.-]+(/[^)]*)?\)' "${PATCH}" | grep -vE '\]\((https?:|#|mailto:)'; then
    echo "Check the relative links above in docs/guide.md: paths are now relative to docs/." >&2
  fi
fi

if git ls-files -u -- README.md | grep -q .; then
  git checkout --ours README.md
  git add README.md
  echo "Kept the fork's README.md."
fi
