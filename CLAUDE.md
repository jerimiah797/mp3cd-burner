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

## Notarization

After building and signing, **always notarize** before creating the DMG. Without notarization, macOS shows a Gatekeeper warning.

Credentials are stored in the keychain under profile `mp3cd-notarize` (set up via `xcrun notarytool store-credentials "mp3cd-notarize"`).

```bash
# 1. Zip the app
ditto -c -k --keepParent "target/release/MP3 CD Burner.app" /tmp/mp3cd-app.zip

# 2. Submit to Apple and wait for approval
xcrun notarytool submit /tmp/mp3cd-app.zip --keychain-profile "mp3cd-notarize" --wait

# 3. Staple the ticket to the app
xcrun stapler staple "target/release/MP3 CD Burner.app"
```

## Creating the DMG

Use `create-dmg` (install with `brew install create-dmg`) to build a polished DMG with background arrow and Applications shortcut. The background image is at `macos/dmg-background.png`.

```bash
create-dmg \
  --volname "MP3 CD Burner" \
  --background macos/dmg-background.png \
  --window-pos 200 120 \
  --window-size 654 422 \
  --icon-size 128 \
  --icon "MP3 CD Burner.app" 175 200 \
  --app-drop-link 475 200 \
  --no-internet-enable \
  target/release/MP3-CD-Burner-X.X.X.dmg \
  "target/release/MP3 CD Burner.app"
```

## Full Release Checklist

1. Bump version in `Cargo.toml` and `macos/Info.plist`
2. `cargo test` - all tests pass
3. Commit and push
4. Tag the release: `git tag vX.X.X && git push origin vX.X.X`
5. `source ~/.zshrc && ./scripts/bundle-macos.sh --universal --sign`
6. Notarize and staple (see above)
7. Create DMG (see above)
8. `gh release create vX.X.X target/release/MP3-CD-Burner-X.X.X.dmg --title "MP3 CD Burner vX.X.X" --notes "..."`
