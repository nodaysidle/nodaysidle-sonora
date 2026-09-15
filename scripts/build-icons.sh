#!/usr/bin/env bash
# Renders src-tauri/icons/logo.svg into the full Tauri icon set.
# Requires: rsvg-convert, iconutil (macOS), ImageMagick.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/src-tauri/icons/logo.svg"
OUT="$ROOT/src-tauri/icons"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

render() { rsvg-convert -w "$1" -h "$1" "$SRC" -o "$2"; }

# Tauri bundle icons
render 32   "$OUT/32x32.png"
render 128  "$OUT/128x128.png"
render 256  "$OUT/128x128@2x.png"
render 256  "$OUT/256x256.png"
render 512  "$OUT/512x512.png"
render 512  "$OUT/icon.png"

# macOS .icns (every size rendered directly from vector, no resampling)
ICONSET="$TMP/Sonora.iconset"
mkdir -p "$ICONSET"
render 16   "$ICONSET/icon_16x16.png"
render 32   "$ICONSET/icon_16x16@2x.png"
render 32   "$ICONSET/icon_32x32.png"
render 64   "$ICONSET/icon_32x32@2x.png"
render 128  "$ICONSET/icon_128x128.png"
render 256  "$ICONSET/icon_128x128@2x.png"
render 256  "$ICONSET/icon_256x256.png"
render 512  "$ICONSET/icon_256x256@2x.png"
render 512  "$ICONSET/icon_512x512.png"
render 1024 "$ICONSET/icon_512x512@2x.png"
iconutil -c icns "$ICONSET" -o "$OUT/icon.icns"

# Windows .ico (multi-resolution)
for s in 16 32 48 64 128 256; do render "$s" "$TMP/ico-$s.png"; done
magick "$TMP/ico-16.png" "$TMP/ico-32.png" "$TMP/ico-48.png" \
       "$TMP/ico-64.png" "$TMP/ico-128.png" "$TMP/ico-256.png" "$OUT/icon.ico"

echo "Icon set written to $OUT"
