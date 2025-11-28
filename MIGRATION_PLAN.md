# MP3 CD Burner: Tauri → GPUI Migration Plan

## Current Tauri Architecture

### Backend Modules (Rust - can be reused)
```
src-tauri/src/
├── audio/
│   ├── mod.rs          - Audio module exports
│   ├── conversion.rs   - FFmpeg encoding strategies
│   ├── detection.rs    - Codec/bitrate detection
│   └── metadata.rs     - Symphonia metadata extraction
├── burning/
│   ├── mod.rs          - Burning module exports
│   ├── cd.rs           - drutil CD burning
│   └── iso.rs          - hdiutil ISO creation
├── profiles/
│   ├── mod.rs          - Profile module exports
│   ├── types.rs        - BurnProfile, BurnSettings structs
│   └── storage.rs      - Save/load/recent profiles
├── utils/
│   ├── mod.rs          - Utils exports
│   ├── types.rs        - State types (SimulateBurnState, etc.)
│   └── helpers.rs      - Helper functions
├── commands/           - Tauri-specific (won't migrate)
│   ├── mod.rs
│   ├── profiles.rs
│   ├── scanning.rs
│   ├── state.rs
│   └── utils.rs
├── menu/               - Tauri-specific (rewrite for GPUI)
│   ├── mod.rs
│   ├── setup.rs
│   └── handlers.rs
└── lib.rs              - Main logic (partially migrate)
```

### Frontend (TypeScript - rewrite in Rust)
```
src/
├── main.ts             - UI logic, event handlers
└── lib/
    ├── burnProfiles.ts - Profile state management
    └── partialEncoding.ts - Encoding optimization logic
```

## Migration Strategy

### Phase 1: Project Structure & Core UI ✅ DONE
- [x] Create gpui project
- [x] Basic window with folder list
- [x] External drag-drop (ExternalPaths)
- [x] Internal drag-drop reordering

### Phase 2: Code Organization ✅ DONE
- [x] Proper module structure (ui/, core/, audio/, burning/, profiles/)
- [x] Separate UI components from business logic
- [x] Extract reusable components (FolderItem, Header, StatusBar)
- [x] Set up testing infrastructure (10 UI/core tests)

### Phase 3: Port Backend Modules ✅ DONE
- [x] Copy `audio/` module (detection, metadata, conversion strategies) - 22 tests
- [x] Copy `burning/` module (iso, cd) - refactored to remove Tauri deps - 2 tests
- [x] Copy `profiles/` module (types, storage) - 3 tests
- [x] Adapt state management (simplified, no Tauri State needed yet)

**Total: 35 tests passing**

### Phase 4: Core Features
- [ ] Folder scanning (scan_music_folder, get_audio_files)
- [ ] Bitrate calculation (the smart encoding logic from main.ts)
- [ ] Progress tracking during conversion
- [ ] Process management (ChildProcesses, CancellationFlag)

### Phase 5: Conversion Pipeline
- [ ] FFmpeg integration (spawn processes, track progress)
- [ ] Parallel conversion with semaphore
- [ ] Smart encoding strategies (copy, convert at source/target bitrate)
- [ ] Album art handling

### Phase 6: CD Burning
- [ ] ISO creation (hdiutil) - backend ready
- [ ] CD burning (drutil) - backend ready
- [ ] CD check dialog loop
- [ ] "Burn Another" mode

### Phase 7: Native Menus ✅ PARTIALLY DONE
- [x] Application menu (About, Quit)
- [x] File menu (structure in place - New, Open, Save, Save As)
- [x] Options menu (structure in place - Simulate, No Lossy, Embed Art)
- [x] Keyboard shortcuts (Cmd+Q)
- [ ] Wire up menu actions to actual functionality
- [ ] Checkmark toggles for Options menu

### Phase 8: Profile System
- [ ] Save/Load profiles with file dialogs
- [ ] Recent profiles in menu
- [ ] Unsaved changes detection

### Phase 9: Polish
- [ ] Album art display
- [ ] Dark mode support
- [ ] Window state persistence
- [ ] Error handling & user feedback

## What Was Directly Copied

These modules had **no Tauri dependencies** and were copied as-is:

1. **`audio/detection.rs`** - Uses symphonia only
2. **`audio/metadata.rs`** - Uses symphonia only
3. **`audio/conversion.rs`** - Pure logic (EncodingStrategy enum)
4. **`profiles/types.rs`** - Pure data structures
5. **`profiles/storage.rs`** - Uses std::fs only

## What Was Refactored

1. **`burning/iso.rs`** - Removed Tauri event emission, returns Result<IsoResult, String>
2. **`burning/cd.rs`** - Removed Tauri deps, uses ProgressCallback for progress reporting

## What Still Needs Work

1. **Folder scanning** - Need to port scan_music_folder logic
2. **FFmpeg conversion** - The main convert_files_background logic from lib.rs
3. **Menu actions** - Wire File/Options menus to actual functionality
4. **File dialogs** - Use GPUI's cx.prompt_for_paths() for Open/Save
5. **Progress UI** - Show conversion/burn progress in the UI

## Key Architectural Differences

| Aspect | Tauri | GPUI |
|--------|-------|------|
| UI | HTML/CSS/JS in WebView | Rust with Tailwind-style API |
| State | Tauri State<T> + JS state | GPUI Entity + Global |
| Events | window.emit() → listen() | cx.notify() / cx.emit() |
| Async | tokio + Tauri async_runtime | GPUI async executor |
| Menus | tauri::menu::* | cx.set_menus() |
| Dialogs | tauri_plugin_dialog | cx.prompt_for_paths() |
| Drag/Drop | WebView drag events | ExternalPaths + typed drags |
