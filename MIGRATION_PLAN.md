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

**Total: 117 tests passing**

### Phase 4: Core Features ✅ DONE
- [x] Folder scanning (scan_music_folder, get_audio_files) - 7 tests
- [x] MusicFolder and AudioFileInfo types
- [x] UI integration (folders scanned on drop, show file count & size)
- [x] Bitrate calculation (smart encoding logic) - 11 tests
- [x] Progress tracking during conversion (real-time UI updates)
- [x] Process management (cancellation support)

### Phase 5: Conversion Pipeline ✅ DONE
- [x] FFmpeg integration (embedded binary, spawn processes)
- [x] Parallel conversion with semaphore (CPU-aware worker pool)
- [x] Smart encoding strategies (copy, convert at source/target bitrate)
- [x] Multi-pass conversion for optimal CD utilization
- [x] Album art extraction (Symphonia → temp files)

### Phase 6: CD Burning ✅ DONE
- [x] ISO creation (hdiutil makehybrid)
- [x] CD burning (hdiutil burn with puppetstrings progress)
- [x] CD detection loop (blank, CD-RW with data, non-erasable)
- [x] CD-RW erase + burn in single operation
- [x] Burn progress tracking with phase detection (erase → burn → finishing)
- [x] Success dialog on completion
- [x] "Burn Another" mode (re-burn same ISO without re-converting)

### Phase 7: Native Menus ✅ DONE
- [x] Application menu (About, Quit)
- [x] File menu (New, Open, Save, Save As - fully wired via focus tree)
- [x] Options menu (Simulate Burn with checkmark toggle)
- [x] Keyboard shortcuts (Cmd+N, Cmd+O, Cmd+S, Cmd+Q)
- [x] Open Output Folder action
- [ ] No Lossy Conversions toggle
- [ ] Embed Album Art toggle

### Phase 8: Profile System ✅ DONE
- [x] Save/Load profiles with file dialogs
- [x] New profile clears folder list
- [ ] Recent profiles in menu
- [ ] Unsaved changes detection

### Phase 9: Polish ✅ MOSTLY DONE
- [x] Album art display in folder cards
- [x] Dark mode support (system appearance detection)
- [x] Minimum window size (500x300)
- [x] Error handling & user feedback (dialogs, progress states)
- [x] Streamlined UI (hidden cancel, consistent button sizing)
- [ ] Window state persistence

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

1. ~~**Folder scanning** - Need to port scan_music_folder logic~~ ✅ DONE
2. ~~**Bitrate calculation** - Smart encoding logic~~ ✅ DONE
3. ~~**FFmpeg conversion** - The main convert_files_background logic from lib.rs~~ ✅ DONE
4. ~~**Progress UI** - Show conversion/burn progress in the UI~~ ✅ DONE
5. ~~**Menu actions** - Wire remaining File/Options menus to actual functionality~~ ✅ DONE
6. ~~**File dialogs** - Use GPUI's cx.prompt_for_paths() for Open/Save profiles~~ ✅ DONE
7. ~~**Profile system** - Save/load burn profiles~~ ✅ DONE
8. ~~**"Burn Another" mode** - Re-burn same ISO without re-converting~~ ✅ DONE
9. **Recent profiles menu** - Show recently opened profiles in File menu
10. **Unsaved changes detection** - Prompt to save before closing/new
11. **No Lossy Conversions toggle** - Options menu toggle
12. **Embed Album Art toggle** - Options menu toggle
13. **Window state persistence** - Remember window position/size

## New Features (Beyond Original Tauri App)

1. ✅ **Background encoding** - Folders encode immediately as added (no waiting for Burn)
2. ✅ **Smart re-encoding** - Lossless files automatically re-encode when bitrate changes
3. ✅ **Priority queue** - Lossy folders encode first, ensuring stable bitrate before lossless

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
