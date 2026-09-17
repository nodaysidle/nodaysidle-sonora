# Hybrid release flow

Local Mac builds the DMG; CI only creates the GitHub Release with generated notes when a version tag is pushed.

1. **Build the DMG locally** on the Mac (existing packaging / `npm run tauri build`).
2. **Tag and push** a version tag (`v*`), e.g. `git tag v0.1.2 && git push origin v0.1.2`.
3. **CI creates the release** (`.github/workflows/release-on-tag.yml`) with generated notes — no binaries.
4. **Attach the asset(s)** with `Scripts/attach-release-asset.sh`:

   ```bash
   Scripts/attach-release-asset.sh v0.1.2 ./path/to/Sonora_0.1.2_aarch64.dmg
   # or multiple:
   Scripts/attach-release-asset.sh v0.1.2 ./path/to/Sonora_0.1.2_aarch64.dmg ./path/to/Sonora_0.1.2_amd64.AppImage
   ```

Existing Latest release is **v0.1.1**. This automation does not republish or replace it; a new `v*` tag is required for a new release.
