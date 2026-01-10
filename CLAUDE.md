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

Use the build script at `scripts/bundle-macos.sh`:

```bash
source ~/.zshrc  # Required to access SIGNING_IDENTITY env var

# Build and sign for current architecture
./scripts/bundle-macos.sh --sign

# Build universal binary (ARM64 + x86_64) and sign
./scripts/bundle-macos.sh --universal --sign
```

The script:
- Builds the release binary with `cargo build --release`
- Creates the .app bundle structure
- Copies ffmpeg, icons, and Info.plist to the bundle
- Signs the app with the Developer ID (requires `SIGNING_IDENTITY` env var)

Output: `target/release/MP3 CD Burner.app`
