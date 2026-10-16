#!/usr/bin/env bash
# Make every file in an AppImage readable (and AppRun/AppRun.wrapped executable)
# for all users, then repack it. Tauri/linuxdeploy can package a cached
# AppRun.wrapped with mode 0770, which fails for non-owner users
# (AppImageHub test: "AppRun.wrapped: Permission denied").
# Usage: scripts/fix-appimage-perms.sh path/to/App.AppImage
set -euo pipefail
img="$(readlink -f "$1")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tool="${APPIMAGETOOL:-$work/appimagetool}"
if [[ ! -x "$tool" ]]; then
  curl -fsSL -o "$tool" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
  chmod +x "$tool"
fi
chmod +x "$img"
(cd "$work" && APPIMAGE_EXTRACT_AND_RUN=1 "$img" --appimage-extract >/dev/null)
chmod -R a+rX "$work/squashfs-root"
chmod a+x "$work/squashfs-root/AppRun"
[[ -e "$work/squashfs-root/AppRun.wrapped" ]] && chmod a+x "$work/squashfs-root/AppRun.wrapped"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream "$work/squashfs-root" "$work/out.AppImage" >/dev/null
mv -f "$work/out.AppImage" "$img"
chmod +x "$img"
echo "fixed permissions: $img"
