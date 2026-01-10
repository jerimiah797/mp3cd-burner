# Claude Code Guidelines for mp3cd-gpui

## Testing Rules

- **Always test before committing.** This means BOTH:
  1. Run `cargo test` and verify all unit tests pass
  2. Run the app with `cargo run` and let the user manually test the changes
- Do not commit until the user confirms the changes work correctly.

## Project Context

This is a GPUI-based MP3 CD burner application. Key points:

- Uses GPUI framework (not Tauri) - run with `cargo run`
- Async work uses `std::thread::spawn` with `tokio::runtime::Runtime::new()` inside, not `tokio::spawn` directly
- Tests: `cargo test` (353+ tests currently)

## Building Signed Releases

**Always build universal binaries** for compatibility with both Intel and Apple Silicon Macs.

Use the build script at `scripts/bundle-macos.sh`:

```bash
source ~/.zshrc  # Required to access SIGNING_IDENTITY env var

# Build universal binary (ARM64 + x86_64) and sign - ALWAYS USE THIS
./scripts/bundle-macos.sh --universal --sign
```

The script:
- Builds release binaries for both aarch64-apple-darwin and x86_64-apple-darwin
- Creates a universal binary with `lipo`
- Creates the .app bundle structure
- Copies ffmpeg, icons, images, and Info.plist to the bundle
- Signs the app with the Developer ID (requires `SIGNING_IDENTITY` env var)

Output: `target/release/MP3 CD Burner.app`

To create a DMG for distribution:
```bash
hdiutil create -volname "MP3 CD Burner" \
  -srcfolder "target/release/MP3 CD Burner.app" \
  -ov -format UDZO \
  target/release/MP3-CD-Burner-X.X.X.dmg
```

## TODO: Notarization

The app is signed but **not yet notarized**. Without notarization, macOS shows a Gatekeeper warning ("Apple cannot check it for malicious software"). Users can bypass by right-clicking → Open, but for proper distribution, notarization is needed:

```bash
# 1. Zip the app
ditto -c -k --keepParent "target/release/MP3 CD Burner.app" /tmp/app.zip

# 2. Submit to Apple (requires app-specific password from appleid.apple.com)
xcrun notarytool submit /tmp/app.zip \
  --apple-id "YOUR_APPLE_ID" \
  --team-id "3QUH73KW5Q" \
  --password "APP_SPECIFIC_PASSWORD" \
  --wait

# 3. Staple the ticket
xcrun stapler staple "target/release/MP3 CD Burner.app"

# 4. Re-create DMG after stapling
```
