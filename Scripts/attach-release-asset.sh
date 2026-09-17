#!/usr/bin/env bash
# Attach one or more locally built DMG/zip assets to an existing GitHub Release for this tag.
# Usage: Scripts/attach-release-asset.sh v0.1.2 ./path/to/Sonora_0.1.2_aarch64.dmg [./path/to/other-asset ...]
set -euo pipefail

TAG="${1:-}"
shift || true
REPO="${REPO:-nodaysidle/nodaysidle-sonora}"

if [[ -z "$TAG" || $# -lt 1 ]]; then
  echo "Usage: $0 <tag> <asset-path> [asset-path ...]" >&2
  exit 1
fi

if ! gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "Release $TAG not found on $REPO. Push the tag first so CI can create the release, or:" >&2
  echo "  gh release create \"$TAG\" --repo \"$REPO\" --generate-notes --title \"Sonora $TAG\"" >&2
  exit 1
fi

for ASSET in "$@"; do
  if [[ ! -f "$ASSET" ]]; then
    echo "Asset not found: $ASSET" >&2
    exit 1
  fi
  gh release upload "$TAG" "$ASSET" --repo "$REPO" --clobber
  echo "Attached $(basename "$ASSET") → $TAG on $REPO"
done
