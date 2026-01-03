# Plan: macOS Bundle Format for .mp3cd Profiles

## Problem Statement

Currently, converted audio files are stored in `/tmp/mp3cd_output/{session_id}/` which:
- Gets deleted on system cleanup/reboot
- Requires re-encoding (several minutes) each time a profile is loaded
- Makes profiles ephemeral despite having "Save" functionality

## Goal

Make `.mp3cd` files into macOS bundles that contain both profile metadata AND converted audio files, enabling persistence without re-conversion.

## What Goes Where

### Bundle (.mp3cd) - PERSISTENT
These files are saved with the profile and persist across sessions:
- `profile.json` - Profile metadata (folder paths, settings, volume label)
- `converted/{folder_id}/*.mp3` - Converted audio files

### Temp Directory (/tmp) - TRANSIENT
These files are regenerated as needed and not persisted:
- `_iso_staging/` - Numbered symlinks for ISO creation
- `*.iso` - Generated ISO files (quick to regenerate from converted files)
- Work-in-progress encoding output (before save)

### Rationale
- **Converting audio: ~3-10 minutes** (expensive, persist this)
- **Generating ISO: ~5-30 seconds** (cheap, regenerate on demand)

## Design Decisions

1. **Bundle Structure** - Directory with `.mp3cd` extension appears as single file in Finder
2. **No Migration** - Old `.mp3cd` JSON files will need manual re-save (acceptable for dev phase)
3. **Profile Size** - 50-700MB is acceptable (CD capacity limited)
4. **Work Directly from Bundle** - After save, encode directly into bundle
5. **ISO in Temp** - Always generate ISO to /tmp, not in bundle

## Bundle Structure

```
MyAlbum.mp3cd/
├── profile.json              # Profile metadata (relative paths)
└── converted/                # Converted audio files
    ├── abc123def456/         # folder_id directory
    │   ├── 01 - Track One.mp3
    │   └── 02 - Track Two.mp3
    └── def789ghi012/
        ├── 01 - Song A.mp3
        └── 02 - Song B.mp3
```

## Implementation Phases

### Phase 1: Info.plist Bundle Declaration

**File:** `macos/Info.plist`

Mark `.mp3cd` as a package so Finder shows it as a single file:
```xml
<key>LSTypeIsPackage</key>
<true/>
```

### Phase 2: Bundle-Aware Profile Storage

**File:** `src/profiles/types.rs`

1. Change `SavedFolderState.output_dir` to store relative path:
   ```rust
   // Before: "/tmp/mp3cd_output/session_123/abc123..."
   // After: "converted/abc123..."  (relative to bundle)
   ```

2. Add version bump to profile format (v2.0 for bundles)

**File:** `src/profiles/storage.rs`

1. Update `save_profile()`:
   - Detect if path ends with `.mp3cd`
   - Create bundle directory structure
   - Write `profile.json` inside bundle
   - (Audio copying handled by OutputManager)

2. Update `load_profile()`:
   - Detect if path is directory (bundle) vs file (legacy JSON)
   - Read `profile.json` from inside bundle
   - Return bundle path for OutputManager to use

3. Update `validate_conversion_state()`:
   - Check for converted audio inside bundle directory
   - Validate source file mtimes match

### Phase 3: OutputManager Bundle Support

**File:** `src/conversion/output_manager.rs`

1. Add `bundle_path: Option<PathBuf>` field
2. Add `set_bundle_path(&mut self, path: Option<&Path>)` method
3. Modify `get_folder_output_dir()`:
   ```rust
   if let Some(bundle) = &self.bundle_path {
       // Return: {bundle}/converted/{folder_id}/
       bundle.join("converted").join(folder_id.as_str())
   } else {
       // Return: /tmp/mp3cd_output/{session_id}/{folder_id}/
       self.session_dir.join(folder_id.as_str())
   }
   ```

4. Add `copy_to_bundle()` method:
   - Copy converted files from temp session to bundle
   - Used during first save

5. Modify cleanup behavior:
   - Never clean up bundle directories
   - Only clean temp session directories

### Phase 4: Save Flow Updates

**File:** `src/ui/components/folder_list/mod.rs`

1. Update `save_profile_dialog()`:
   - Create bundle directory at save path
   - If first save (files in temp): copy converted files to bundle
   - Set `output_manager.bundle_path` to bundle
   - Update folder states with relative paths
   - Write profile.json

2. If re-saving to same bundle:
   - Files already in bundle, no copy needed
   - Just update profile.json

### Phase 5: Load Flow Updates

**File:** `src/ui/components/folder_list/mod.rs`

1. Update `load_profile()`:
   - Detect bundle vs legacy JSON
   - Set `output_manager.bundle_path` to bundle
   - Resolve relative paths to absolute for validation
   - Validate converted files exist in bundle
   - Restore folder conversion status (skip re-encoding!)

### Phase 6: Background Encoder Integration

**File:** `src/conversion/background.rs`

1. Encoder gets output path from OutputManager
2. OutputManager returns bundle path when set
3. New conversions go directly into bundle (after first save)

## Files to Modify

| File | Changes |
|------|---------|
| `macos/Info.plist` | Add `LSTypeIsPackage = true` |
| `src/profiles/types.rs` | Version bump, document relative paths |
| `src/profiles/storage.rs` | Bundle detection, bundle read/write |
| `src/conversion/output_manager.rs` | Bundle path support, copy_to_bundle |
| `src/ui/components/folder_list/mod.rs` | Save/load flow for bundles |
| `src/conversion/background.rs` | Pass bundle awareness through encoder |

## Data Flow

### Initial Encode (No Bundle Yet)
```
User adds folders → Background encoder runs
                         ↓
              Files go to /tmp/mp3cd_output/{session}/{folder_id}/
                         ↓
              User can burn CD from temp files
```

### Save Flow (First Save)
```
User clicks Save → save_profile_dialog()
                        ↓
              User selects MyAlbum.mp3cd
                        ↓
              Create MyAlbum.mp3cd/ directory
                        ↓
              Copy /tmp/.../converted files → MyAlbum.mp3cd/converted/
                        ↓
              output_manager.set_bundle_path(bundle)
                        ↓
              Build profile with relative paths
                        ↓
              Write MyAlbum.mp3cd/profile.json
                        ↓
              Future encodes go directly to bundle
```

### Load Flow
```
User opens MyAlbum.mp3cd/ → load_profile()
                                ↓
              Detect it's a bundle (directory)
                                ↓
              Read MyAlbum.mp3cd/profile.json
                                ↓
              output_manager.set_bundle_path(bundle)
                                ↓
              Validate MyAlbum.mp3cd/converted/* exists
                                ↓
              Restore folder states → Shows as "Converted" ✓
                                ↓
              No re-encoding needed!
```

### Burn Flow (From Bundle)
```
User clicks Burn → ISO generation needed
                        ↓
              Create /tmp/.../iso_staging/ with symlinks
              pointing to MyAlbum.mp3cd/converted/*/
                        ↓
              Generate ISO to /tmp/.../*.iso
                        ↓
              Burn ISO to CD
```

## Edge Cases

1. **Save with encoding in progress** - Wait for completion or save partial state
2. **Source files changed** - Detect via mtime, queue for re-encoding into bundle
3. **Bundle already open** - Overwrite in place (update profile.json, add new folders)
4. **Disk space** - User's responsibility (up to 700MB per bundle)
5. **Legacy JSON profiles** - Won't load (or could add migration path later)
6. **Folder removed from profile** - Delete its converted/ subfolder from bundle

## Success Criteria

- [ ] `.mp3cd` bundles appear as single files in Finder
- [ ] Opening a saved bundle shows all folders as "Converted" immediately
- [ ] No re-encoding happens when opening a bundle with valid audio
- [ ] Profile + audio travel together when moving/copying the bundle
- [ ] ISO is generated to temp, not stored in bundle
- [ ] New encoding after save goes directly into bundle
