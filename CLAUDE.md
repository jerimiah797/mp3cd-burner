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
- Tests: `cargo test` (117+ tests currently)
