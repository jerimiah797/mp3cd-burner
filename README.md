# MP3 CD Burner

A native macOS application for creating MP3 CDs from your music library. Drag folders of music, and the app automatically converts everything to MP3 format optimized to fit on a 700MB CD.

## Overview

MP3 CD Burner takes the hassle out of creating MP3 CDs:

- **Smart bitrate calculation** - Automatically determines the optimal bitrate to maximize audio quality while fitting on a standard 700MB CD
- **Background encoding** - Files start converting immediately as you add folders, no waiting
- **Lossless-aware** - FLAC and WAV files are converted to MP3; existing MP3s are copied directly when possible
- **One-click burning** - When encoding completes, just click "Convert & Burn" to create your CD

## Supported Formats

| Format | Type | Handling |
|--------|------|----------|
| MP3 | Lossy | Copied directly |
| FLAC | Lossless | Converted to MP3 at calculated bitrate |
| WAV | Lossless | Converted to MP3 at calculated bitrate |
| AIFF | Lossless | Converted to MP3 at calculated bitrate |
| AAC/M4A | Lossy | Transcoded to MP3 at source bitrate |
| OGG | Lossy | Transcoded to MP3 at source bitrate |
| Opus | Lossy | Transcoded to MP3 at source bitrate |
| ALAC | Lossless | Converted to MP3 at calculated bitrate |

## How to Use

### 1. Add Music Folders

Drag album folders from Finder into the app window. The app will:
- Scan each folder for audio files
- Extract album metadata (artist, album name, year) from tags
- Begin converting files in the background immediately

You can also drag a parent folder containing multiple albums - the app will detect and add each album separately.

### Create Mixtapes

Create custom playlists by dragging individual audio files (not folders) into the app. This creates a "Mixtape" that you can name and customize:
- Drag audio files from different albums into the app
- A new mixtape is created automatically
- Double-click to open the Track Editor to rename, reorder, or add more tracks

### Track Editor

Double-click any folder to open the Track Editor window:

**For Albums:**
- View all tracks with metadata (title, artist, duration)
- Drag tracks to reorder them
- Click the checkbox to exclude/include individual tracks
- Excluded tracks are dimmed and won't be burned to CD

**For Mixtapes:**
- All album features plus:
- Drag additional audio files from Finder to add them
- Click the × button to remove tracks
- Edit the mixtape name in the title bar

### 2. Monitor Progress

As folders are added:
- Each folder shows a progress bar during encoding
- The status bar shows total files, duration, and source size
- The calculated bitrate updates based on total content
- ISO size appears once all folders are encoded

### 3. Watch the ISO Size

The status bar shows:
- **Source**: Total size of your original files
- **Target**: 700 MB (standard CD capacity)
- **Bitrate**: Calculated optimal bitrate (or your manual override)
- **ISO**: Final ISO size after encoding

If the ISO exceeds 700 MB, you'll need to remove some folders, or perhaps customize the bitrate on lossless files.

### 4. Adjust Bitrate (Optional)

Click on the bitrate display to manually override it. This lets you:
- Increase bitrate if you have room to spare
- Decrease bitrate to fit more music
- The asterisk (e.g., "285 kbps*") indicates a manual override

When you change the bitrate, lossless files are automatically re-encoded.

If the calculated bitrate produces an ISO with extra space, you can increase the bitrate and re-encode to maximize quality. Adjust until the ISO size is as close to 700 MB as desired without exceeding it.

### 5. Burn Your CD

Once all folders show green checkmarks:
1. Click **Convert & Burn**
2. Enter a volume label for your CD
3. Insert a blank CD-R or CD-RW when prompted
4. Wait for burning to complete

After burning, click **Burn Another** to make additional copies without re-encoding.

## Burn Profiles

Save your folder list and settings for later use.

### Saving Profiles

**File > Save** (Cmd+S) offers two options:

| Option | File Size | Use Case |
|--------|-----------|----------|
| **Metadata Only** | ~1 KB | Quick save; files will re-encode when reopened |
| **Include Audio Files** | ~600+ MB | Portable bundle; no re-encoding needed |

### Metadata Only (.mp3cd file)

- Saves folder paths, volume label, and bitrate settings
- Very small file size
- When reopened, validates that source folders still exist
- Re-encodes any folders that have changed

### Include Audio Files (.mp3cd bundle)

- Creates a folder bundle containing the profile and all converted MP3s
- Large file size (similar to final ISO)
- Completely portable - can be moved to another Mac
- Opens instantly with no re-encoding needed
- Ideal for archiving a "ready to burn" state

### What's Saved in Profiles

- Folder paths and order
- Volume label (if set)
- Manual bitrate override (if set)
- Conversion state (which folders are already encoded)
- ISO state (if an ISO has been generated)

## Menu Options

### File Menu
- **New** (Cmd+N) - Clear all folders and start fresh
- **New Mixtape** (Cmd+Shift+N) - Create an empty mixtape to add tracks to
- **Open** (Cmd+O) - Open a saved burn profile
- **Save** (Cmd+S) - Save current state as a burn profile

### Edit Menu
- **Set Volume Label** - Change the CD volume label

### View Menu
- **Display Settings** - Toggle which metadata appears on folder cards:
  - File count
  - Original size
  - Converted size
  - Source format
  - Source bitrate
  - Final bitrate

### Options Menu
- **Simulate Burn** - Test the burn process without using a disc
- **Embed Album Art** - Include cover art in output MP3 files
- **Open Output Folder** - Reveal the temporary encoding directory

### Help Menu
- **Open Log Folder** - Reveal the log directory for troubleshooting

## Technical Details

### Architecture

Built with [GPUI](https://gpui.rs), Zed's native Rust UI framework. The app uses:
- **Global file queue** - All tracks across all folders are queued together for maximum throughput
- **Parallel encoding** - 2-8 worker threads (based on available CPU cores) pull from the shared queue
- **FFmpeg** for all audio conversion (bundled with the app)
- **hdiutil** for ISO creation and CD burning (macOS built-in)

### Encoding Strategy

The app uses a two-phase approach with intelligent file handling:

**Phase 1 - Lossy files:**
- All lossy files from all folders are queued together
- MP3s are copied directly (preserving original quality)
- Other lossy formats (AAC, OGG, Opus) are transcoded at their source bitrate

**Phase 2 - Lossless files:**
- After Phase 1 completes, the app measures total lossy output size
- Calculates optimal bitrate for lossless files to maximize quality while fitting on CD
- All lossless files (FLAC, WAV, AIFF, ALAC) are queued and encoded at the calculated bitrate

This ensures maximum audio quality: lossy files keep their original quality, while lossless files get the highest bitrate that will fit.

*Future: Re-encode high-bitrate MP3s when necessary to fit on CD.*

### Smart MP3 Handling

MP3 files are copied directly to preserve original quality. When the "Embed Album Art" option is enabled, MP3s without embedded artwork are re-encoded to include the album's cover art.

### CD Burning

Uses macOS native tools:
- `hdiutil makehybrid` - Creates ISO-9660 + Joliet hybrid image
- `hdiutil burn` - Burns with progress tracking via puppetstrings

CD-RW discs are detected and can be erased before burning.

### File Locations

- **Temporary files**: `/tmp/mp3cd_output/session_*/`
- **Profiles**: Saved wherever you choose (Documents recommended)
- **Settings**: `~/Library/Application Support/mp3cd-gpui/`
- **Logs**: `~/Library/Logs/MP3-CD-Burner/mp3cd-burner.log`

The log file captures debug-level information and can be helpful for troubleshooting. Logs rotate automatically when they exceed 10MB.

## Requirements

- macOS 11.0 or later
- Optical drive (internal or external USB)
- Blank CD-R or CD-RW discs

## Building from Source

```bash
# Clone the repository
git clone <repo-url>
cd mp3cd-gpui

# Build and run
cargo run

# Run tests
cargo test

# Build release
cargo build --release
```

## License

[Add your license here]

## Acknowledgments

- [GPUI](https://gpui.rs) - Native Rust UI framework by Zed Industries
- [FFmpeg](https://ffmpeg.org) - Audio conversion (LGPL)
- [Symphonia](https://github.com/pdeljanov/Symphonia) - Audio metadata extraction
