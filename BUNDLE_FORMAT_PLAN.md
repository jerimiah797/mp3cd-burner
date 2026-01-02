# Plan: macOS Bundle Format for .burn Profiles

## Problem Statement

Currently, converted audio files are stored in `/tmp/mp3cd_output/{session_id}/` which:
- Gets deleted on system cleanup/reboot
- Requires re-encoding (several minutes) each time a profile is loaded
- Makes profiles ephemeral despite having "Save" functionality

## Goal

Make `.burn` files into macOS bundles that contain both profile metadata AND converted audio files, enabling persistence without re-conversion.

## Design Decisions

1. **Bundle Structure** - Directory with `.burn` extension appears as single file in Finder
2. **No Migration** - Old `.burn` JSON files will fail to load (acceptable)
3. **Profile Size** - 50-700MB is acceptable (CD capacity limited)
4. **Work Directly from Bundle** - No copying to temp; encode/read directly from bundle

## Bundle Structure

```
MyAlbum.burn/
├── profile.json              # Profile metadata (relative paths)
└── converted/                # Converted audio files
    ├── abc123def456.../      # folder_id directory
    │   ├── track01.mp3
    │   └── track02.mp3
    └── def789ghi012.../
        ├── track01.mp3
        └── track02.mp3
```

## Implementation Phases

### Phase 1: Bundle-Aware Profile Storage

**File:** `src/profiles/types.rs`

1. Change `SavedFolderState.output_dir` to store relative path:
   ```rust
   // Before: "/tmp/mp3cd_output/session_123/abc123..."
   // After: "converted/abc123..."
   ```

2. Add version bump to profile format (v1.2 for bundles)

**File:** `src/profiles/storage.rs`

1. Update `save_profile()`:
   - Create bundle directory structure
   - Write `profile.json` inside bundle
   - Copy converted audio from temp to `{bundle}/converted/`

2. Update `load_profile()`:
   - Detect if path is directory (bundle) vs file (legacy)
   - Read `profile.json` from inside bundle
   - Resolve relative paths to absolute bundle paths

3. Update `validate_conversion_state()`:
   - Check for converted audio inside bundle directory
   - Validate source file mtimes match

### Phase 2: OutputManager Bundle Support

**File:** `src/conversion/output_manager.rs`

1. Add `bundle_path: Option<PathBuf>` field
2. Add `OutputManager::set_bundle_path(&mut self, path: &Path)`
3. Modify `get_folder_output_dir()`:
   - If bundle_path is set: return `{bundle}/converted/{folder_id}/`
   - Otherwise: return `/tmp/mp3cd_output/{session_id}/{folder_id}/`

4. Modify cleanup behavior:
   - Don't clean up bundle directories
   - Only clean temp session directories

### Phase 3: Save Flow Updates

**File:** `src/ui/components/folder_list.rs`

1. Update `save_profile()`:
   - Create bundle directory at save path
   - Set `output_manager.bundle_path` to bundle
   - Move converted files from temp to bundle
   - Update folder states with relative paths
   - Write profile.json

2. If re-saving to same bundle:
   - Update only changed folders
   - Preserve existing converted audio

### Phase 4: Load Flow Updates

**File:** `src/ui/components/folder_list.rs`

1. Update `load_profile()`:
   - Detect bundle vs legacy JSON
   - Set `output_manager.bundle_path` to bundle
   - Resolve relative paths to absolute
   - Validate converted files exist in bundle
   - Skip re-encoding for valid folders

### Phase 5: Background Encoder Integration

**File:** `src/conversion/background.rs`

1. Pass bundle awareness through encoder:
   - Encoder gets output path from OutputManager
   - OutputManager returns bundle path when set
   - Converted files go directly into bundle

## Files to Modify

| File | Changes |
|------|---------|
| `src/profiles/types.rs` | Version bump, relative path handling |
| `src/profiles/storage.rs` | Bundle detection, bundle read/write |
| `src/conversion/output_manager.rs` | Bundle path support, path resolution |
| `src/ui/components/folder_list.rs` | Save/load flow for bundles |

## Data Flow

### Save Flow
```
User clicks Save → save_profile_dialog()
                        ↓
              User selects MyAlbum.burn
                        ↓
              Create MyAlbum.burn/ directory
                        ↓
              output_manager.set_bundle_path(bundle)
                        ↓
              Copy /tmp/mp3cd_output/.../converted files
                  → MyAlbum.burn/converted/
                        ↓
              Build profile with relative paths
                        ↓
              Write MyAlbum.burn/profile.json
```

### Load Flow
```
User clicks Open → open_profile()
                        ↓
              User selects MyAlbum.burn/
                        ↓
              Detect it's a bundle (directory)
                        ↓
              Read MyAlbum.burn/profile.json
                        ↓
              output_manager.set_bundle_path(bundle)
                        ↓
              Validate MyAlbum.burn/converted/* exists
                        ↓
              Restore folder states (no re-encoding!)
```

## Edge Cases

1. **Save with encoding in progress** - Wait for completion or cancel
2. **Source files changed** - Detect via mtime, queue for re-encoding into bundle
3. **Bundle already open** - Overwrite in place
4. **Disk space** - User's responsibility (up to 700MB per bundle)

## Success Criteria

- Opening a saved `.burn` bundle shows all folders as "Converted"
- No re-encoding happens when opening a bundle with valid audio
- Bundles appear as single files in Finder
- Profile + audio travel together when moving/copying the bundle
