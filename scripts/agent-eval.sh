#!/usr/bin/env bash
# Before/after comparison of what agents get from real captures, for one on/off setting.
#
#   scripts/agent-eval.sh [--setting agentContext] [--out DIR] [bundle…]
#
# Reprocesses each bundle twice with target/debug/scrawl (build it first): once with the setting
# off and once with it on (for the images), then writes DIR/<bundle>/
# {before,after}.md (both from the baseline run's transcript), each run's contact sheet and
# frames, and DIR/index.md with both summaries one after the other.
# The original bundles are only read. Without bundle arguments it uses a default set of Kit's
# captures. Facts recorded live (Accessibility lookups, sdk.js page events) can't be replayed:
# reprocess keeps whatever the capture recorded.
set -euo pipefail

setting="agentContext"
out="$HOME/code/home/artifacts/scrawl-context"
bundles=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --setting) setting="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    *) bundles+=("$1"); shift ;;
  esac
done

captures="$HOME/Library/Application Support/scrawl/captures"
if [[ ${#bundles[@]} -eq 0 ]]; then
  for name in \
    scrawl-1792430714486-11a5e-13 \
    scrawl-1792431401417-11a5e-15 \
    scrawl-1792431694891-11a5e-17 \
    scrawl-1792432139176-11a5e-19 \
    scrawl-1792390827408-a783-24 \
    scrawl-1792391130438-a783-28 \
    scrawl-1792432960874-11a5e-21; do
    bundles+=("$captures/$name.scrawl")
  done
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
scrawl="$root/target/debug/scrawl"
[[ -x "$scrawl" ]] || { echo "build first: cargo build" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/scrawl-agent-eval.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# Kit's own config with the setting off (before) and on (after).
for variant in before after; do
  python3 - "$HOME/.config/scrawl/config.json" "$work/$variant.json" "$setting" "$variant" <<'PY'
import json, sys
try:
    data = json.load(open(sys.argv[1]))
except Exception:
    data = {}
data[sys.argv[3]] = sys.argv[4] == "after"
json.dump(data, open(sys.argv[2], "w"))
PY
done

mkdir -p "$out"
index="$out/index.md"
{
  echo "# Scrawl setting \`$setting\`: before and after"
  echo
  echo "Generated $(date '+%Y-%m-%d %H:%M') from $(git -C "$root" rev-parse --short HEAD). Each bundle was reprocessed from its own video, audio and input log twice, with \`$setting\` off and on."
} > "$index"

for bundle in "${bundles[@]}"; do
  name="$(basename "$bundle" .scrawl)"
  dest="$out/$name"
  mkdir -p "$dest"
  for variant in before after; do
    run="$work/$name-$variant.scrawl"
    args=(reprocess "$bundle" --out "$run")
    if ! SCRAWL_CONFIG="$work/$variant.json" "$scrawl" "${args[@]}" > "$work/$name-$variant.log" 2>&1; then
      echo "$name: $variant failed (see below)" >&2
      tail -5 "$work/$name-$variant.log" >&2
      continue 2
    fi
    # Both summaries come from the baseline run, so a second transcription pass (which can word
    # things slightly differently) doesn't show up as a difference. The header points at the
    # original bundle, not the scratch copy.
    base="$work/$name-before.scrawl"
    if [[ "$variant" == before ]]; then
      sed "s|$run|$bundle|g" "$run/summary.md" > "$dest/$variant.md"
    else
      SCRAWL_CONFIG="$work/after.json" "$scrawl" inspect "$base" | sed "s|$base|$bundle|g" > "$dest/$variant.md"
    fi
    cp "$run/contact-sheet.png" "$dest/$variant-contact-sheet.png"
    rm -rf "$dest/$variant-frames"
    cp -R "$run/frames" "$dest/$variant-frames"
  done
  {
    echo
    echo "## $name"
    echo
    echo "Contact sheets: [before]($name/before-contact-sheet.png) · [after]($name/after-contact-sheet.png)"
    echo
    echo "Before:"
    echo
    echo '```'
    cat "$dest/before.md"
    echo '```'
    echo
    echo "After:"
    echo
    echo '```'
    cat "$dest/after.md"
    echo '```'
  } >> "$index"
  echo "$name: done"
done
echo "Wrote $index"
