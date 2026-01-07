//! Folder scanning and audio file discovery
//!
//! This module provides functions for scanning music folders, discovering
//! audio files, and collecting metadata about them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::audio::{get_album_art, get_album_metadata, get_audio_metadata, is_audio_file};
use crate::core::folder_state::{FolderConversionStatus, FolderId};

/// Represents metadata about a music folder
#[derive(Debug, Clone)]
pub struct MusicFolder {
    /// Unique identifier for this folder (based on path + mtime, or UUID for mixtapes)
    pub id: FolderId,
    pub path: PathBuf,
    pub file_count: u32,
    pub total_size: u64,
    pub total_duration: f64,
    pub album_art: Option<String>,
    /// Album name from audio file metadata
    pub album_name: Option<String>,
    /// Artist name from audio file metadata
    pub artist_name: Option<String>,
    /// Release year from audio file metadata
    pub year: Option<String>,
    /// Cached audio file info for bitrate calculation
    pub audio_files: Vec<AudioFileInfo>,
    /// Current conversion status for background encoding
    pub conversion_status: FolderConversionStatus,
    /// Whether the source files are accessible (false if loaded from bundle with missing source)
    /// When false, the folder can still be burned but cannot be re-encoded at a different bitrate.
    pub source_available: bool,
    /// The kind of folder (Album or Mixtape)
    pub kind: FolderKind,
    /// Tracks excluded from the burn (by path)
    pub excluded_tracks: Vec<PathBuf>,
    /// Custom track order (indices into audio_files). None means original order.
    pub track_order: Option<Vec<usize>>,
}

impl MusicFolder {
    /// Returns true if this folder contains any lossless audio files
    pub fn has_lossless_files(&self) -> bool {
        self.audio_files.iter().any(|f| !f.is_lossy)
    }

    /// Returns a summary of source formats (e.g., "FLAC" or "MP3/AAC")
    pub fn source_format_summary(&self) -> String {
        use std::collections::HashSet;

        let formats: HashSet<&str> = self
            .audio_files
            .iter()
            .map(|f| Self::normalize_codec(&f.codec))
            .collect();

        if formats.is_empty() {
            return String::new();
        }

        // Sort for consistent display
        let mut formats: Vec<&str> = formats.into_iter().collect();
        formats.sort();
        formats.join("/")
    }

    /// Normalize codec names for display
    fn normalize_codec(codec: &str) -> &'static str {
        let codec_lower = codec.to_lowercase();
        if codec_lower.contains("flac") {
            "FLAC"
        } else if codec_lower.contains("mp3") || codec_lower.contains("mpeg") {
            "MP3"
        } else if codec_lower.contains("aac") || codec_lower.contains("m4a") {
            "AAC"
        } else if codec_lower.contains("wav") || codec_lower.contains("pcm") {
            "WAV"
        } else if codec_lower.contains("ogg") || codec_lower.contains("vorbis") {
            "OGG"
        } else if codec_lower.contains("opus") {
            "OPUS"
        } else if codec_lower.contains("alac") {
            "ALAC"
        } else {
            "Other"
        }
    }

    /// Returns a summary of source bitrates (e.g., "320k" or "128-320k")
    pub fn source_bitrate_summary(&self) -> String {
        if self.audio_files.is_empty() {
            return String::new();
        }

        // Only include bitrates from lossy files (lossless bitrates are meaningless)
        let lossy_bitrates: Vec<u32> = self
            .audio_files
            .iter()
            .filter(|f| f.is_lossy && f.bitrate > 0)
            .map(|f| f.bitrate)
            .collect();

        if lossy_bitrates.is_empty() {
            // All files are lossless
            return "lossless".to_string();
        }

        let min = lossy_bitrates.iter().min().copied().unwrap_or(0);
        let max = lossy_bitrates.iter().max().copied().unwrap_or(0);

        if min == max {
            format!("{}k", min)
        } else {
            format!("{}-{}k", min, max)
        }
    }

    /// Returns true if this folder is a mixtape
    pub fn is_mixtape(&self) -> bool {
        matches!(self.kind, FolderKind::Mixtape { .. })
    }

    /// Returns the mixtape name if this is a mixtape, None otherwise
    pub fn mixtape_name(&self) -> Option<&str> {
        match &self.kind {
            FolderKind::Mixtape { name } => Some(name),
            FolderKind::Album => None,
        }
    }

    /// Returns a display name for this folder
    ///
    /// For mixtapes: returns the mixtape name
    /// For albums: returns album_name if available, otherwise folder name
    pub fn display_name(&self) -> String {
        match &self.kind {
            FolderKind::Mixtape { name } => name.clone(),
            FolderKind::Album => self
                .album_name
                .clone()
                .unwrap_or_else(|| {
                    self.path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Unknown".to_string())
                }),
        }
    }

    /// Sets the mixtape name (only works for mixtape folders)
    pub fn set_mixtape_name(&mut self, new_name: String) {
        if let FolderKind::Mixtape { name } = &mut self.kind {
            *name = new_name;
        }
    }

    /// Returns tracks to encode in order (respects track_order and excludes excluded_tracks)
    pub fn active_tracks(&self) -> Vec<&AudioFileInfo> {
        let ordered: Vec<&AudioFileInfo> = match &self.track_order {
            Some(order) => order
                .iter()
                .filter_map(|&i| self.audio_files.get(i))
                .collect(),
            None => self.audio_files.iter().collect(),
        };
        ordered
            .into_iter()
            .filter(|f| !self.excluded_tracks.contains(&f.path))
            .collect()
    }

    /// Exclude a track from the burn
    pub fn exclude_track(&mut self, path: &Path) {
        if !self.excluded_tracks.contains(&path.to_path_buf()) {
            self.excluded_tracks.push(path.to_path_buf());
        }
    }

    /// Re-include a previously excluded track
    pub fn include_track(&mut self, path: &Path) {
        self.excluded_tracks.retain(|p| p != path);
    }

    /// Set a custom track order (indices into audio_files)
    pub fn set_track_order(&mut self, order: Vec<usize>) {
        self.track_order = Some(order);
    }

    /// Reset to original track order
    pub fn reset_track_order(&mut self) {
        self.track_order = None;
    }

    /// Recalculate file_count, total_size, and total_duration from audio_files
    pub fn recalculate_totals(&mut self) {
        self.file_count = self.audio_files.len() as u32;
        self.total_size = self.audio_files.iter().map(|f| f.size).sum();
        self.total_duration = self.audio_files.iter().map(|f| f.duration).sum();
    }

    /// Create a new empty mixtape folder
    pub fn new_mixtape(name: String, audio_files: Vec<AudioFileInfo>) -> Self {
        let file_count = audio_files.len() as u32;
        let total_size: u64 = audio_files.iter().map(|f| f.size).sum();
        let total_duration: f64 = audio_files.iter().map(|f| f.duration).sum();

        Self {
            id: FolderId::new_mixtape(),
            path: PathBuf::new(), // Mixtapes don't have a path
            file_count,
            total_size,
            total_duration,
            album_art: None,
            album_name: None,
            artist_name: None,
            year: None,
            audio_files,
            conversion_status: FolderConversionStatus::default(),
            source_available: true,
            kind: FolderKind::Mixtape { name },
            excluded_tracks: Vec::new(),
            track_order: None,
        }
    }
}

#[cfg(test)]
impl MusicFolder {
    /// Create a MusicFolder for testing purposes
    ///
    /// Creates a folder with sensible defaults:
    /// - 10 files, 50MB total, 40 minutes duration
    /// - No album art, empty audio files list
    /// - Default conversion status
    pub fn new_for_test(path: &str) -> Self {
        Self {
            id: FolderId::from_path(Path::new(path)),
            path: PathBuf::from(path),
            file_count: 10,
            total_size: 50_000_000,
            total_duration: 2400.0, // 40 minutes
            album_art: None,
            album_name: None,
            artist_name: None,
            year: None,
            audio_files: Vec::new(),
            conversion_status: FolderConversionStatus::default(),
            source_available: true,
            kind: FolderKind::Album,
            excluded_tracks: Vec::new(),
            track_order: None,
        }
    }

    /// Create a MusicFolder for testing with a custom name-based ID
    ///
    /// Uses the name directly as the FolderId (useful for hash testing)
    pub fn new_for_test_with_id(name: &str) -> Self {
        Self {
            id: FolderId(name.to_string()),
            path: PathBuf::from(format!("/test/{}", name)),
            file_count: 5,
            total_size: 50_000_000,
            total_duration: 300.0,
            album_art: None,
            album_name: None,
            artist_name: None,
            year: None,
            audio_files: Vec::new(),
            conversion_status: FolderConversionStatus::default(),
            source_available: true,
            kind: FolderKind::Album,
            excluded_tracks: Vec::new(),
            track_order: None,
        }
    }
}

/// Represents metadata about an audio file
#[derive(Debug, Clone)]
pub struct AudioFileInfo {
    pub path: PathBuf,
    pub duration: f64,
    pub bitrate: u32,
    pub size: u64,
    pub codec: String,
    pub is_lossy: bool,
}

/// The kind of folder - either a scanned album or a user-created mixtape
#[derive(Debug, Clone, Default)]
pub enum FolderKind {
    /// A folder scanned from the filesystem (traditional album)
    #[default]
    Album,
    /// A user-created mixtape/playlist with a custom name
    Mixtape { name: String },
}

/// Scan a music folder and get basic metadata
///
/// Returns a MusicFolder with file count, total size, duration, album art, and cached audio files.
pub fn scan_music_folder(path: &Path) -> Result<MusicFolder, String> {
    if !path.is_dir() {
        return Err(format!("Path is not a directory: {}", path.display()));
    }

    // Get all audio files with full metadata (handles deduplication)
    let audio_files = get_audio_files(path)?;

    // Calculate summary stats from cached files
    let file_count = audio_files.len() as u32;
    let total_size: u64 = audio_files.iter().map(|f| f.size).sum();
    let total_duration: f64 = audio_files.iter().map(|f| f.duration).sum();

    // Extract album art and metadata from the first audio file
    let (album_art, album_name, artist_name, year) = if let Some(first_file) = audio_files.first() {
        let art = get_album_art(&first_file.path);
        let metadata = get_album_metadata(&first_file.path);
        (art, metadata.album, metadata.artist, metadata.year)
    } else {
        (None, None, None, None)
    };

    // Generate unique folder ID based on path and modification time
    let id = FolderId::from_path(path);

    Ok(MusicFolder {
        id,
        path: path.to_path_buf(),
        file_count,
        total_size,
        total_duration,
        album_art,
        album_name,
        artist_name,
        year,
        audio_files,
        conversion_status: FolderConversionStatus::default(),
        source_available: true, // Scanned from source, so it's available
        kind: FolderKind::Album,
        excluded_tracks: Vec::new(),
        track_order: None,
    })
}

/// Create a MusicFolder from saved profile metadata (when source is unavailable)
///
/// This is used when loading a bundle profile where the source folder is missing.
/// The folder can still be burned but cannot be re-encoded.
pub fn create_folder_from_metadata(
    folder_id: String,
    path: PathBuf,
    file_count: u32,
    total_size: u64,
    total_duration: f64,
    album_name: Option<String>,
    artist_name: Option<String>,
    year: Option<String>,
    album_art: Option<String>,
    conversion_status: FolderConversionStatus,
    kind: Option<FolderKind>,
    excluded_tracks: Option<Vec<PathBuf>>,
    track_order: Option<Vec<usize>>,
) -> MusicFolder {
    MusicFolder {
        id: FolderId(folder_id),
        path,
        file_count,
        total_size,
        total_duration,
        album_art,
        album_name,
        artist_name,
        year,
        audio_files: Vec::new(), // No source files to scan
        conversion_status,
        source_available: false, // Created from metadata, source not available
        kind: kind.unwrap_or(FolderKind::Album),
        excluded_tracks: excluded_tracks.unwrap_or_default(),
        track_order,
    }
}

/// Find all "album folders" under a given path
///
/// An album folder is a directory that contains audio files directly.
/// This function handles smart expansion:
/// - If the path contains audio files directly, returns just that path
/// - If the path only contains subdirectories, finds all descendant folders
///   that contain audio files and returns those
///
/// This allows users to drag a parent folder (e.g., /Artist/) and have each
/// album subfolder imported separately.
pub fn find_album_folders(path: &Path) -> Vec<PathBuf> {
    if !path.is_dir() {
        return vec![];
    }

    // Check if this folder directly contains audio files
    let has_direct_audio = fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.path().is_file() && is_audio_file(&e.path()))
        })
        .unwrap_or(false);

    if has_direct_audio {
        // This folder has audio files - import it as-is
        return vec![path.to_path_buf()];
    }

    // No direct audio files - look for subfolders that contain audio
    let mut album_folders = Vec::new();

    // Use WalkDir to find all directories, then check each for audio files
    for entry in WalkDir::new(path)
        .follow_links(true)
        .min_depth(1) // Skip the root path itself
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            // Check if this subdirectory has audio files directly
            let subdir_has_audio = fs::read_dir(entry_path)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .any(|e| e.path().is_file() && is_audio_file(&e.path()))
                })
                .unwrap_or(false);

            if subdir_has_audio {
                album_folders.push(entry_path.to_path_buf());
            }
        }
    }

    // Sort for consistent ordering
    album_folders.sort();
    album_folders
}

/// Get all audio files in a directory with full metadata
///
/// This function handles deduplication of files that appear in both
/// a parent directory and a subdirectory (e.g., mp3/ or flac/ subdirs).
pub fn get_audio_files(path: &Path) -> Result<Vec<AudioFileInfo>, String> {
    if !path.is_dir() {
        return Err(format!("Path is not a directory: {}", path.display()));
    }

    let mut files = Vec::new();
    let mut file_stems_by_dir: HashMap<PathBuf, HashSet<String>> = HashMap::new();

    // First pass: collect all file stems organized by their parent directory
    for entry in WalkDir::new(path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path_buf = entry.path().to_path_buf();
        if path_buf.is_file() && is_audio_file(&path_buf)
            && let Some(parent) = path_buf.parent()
                && let Some(stem) = path_buf.file_stem().and_then(|s| s.to_str()) {
                    file_stems_by_dir
                        .entry(parent.to_path_buf())
                        .or_default()
                        .insert(stem.to_string());
                }
    }

    // Second pass: collect files, but skip subdirectory files that duplicate parent stems
    for entry in WalkDir::new(path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path_buf = entry.path().to_path_buf();
        if path_buf.is_file() && is_audio_file(&path_buf) {
            // Check if this file is in a subdirectory and duplicates a parent file
            if let Some(parent) = path_buf.parent()
                && let Some(stem) = path_buf.file_stem().and_then(|s| s.to_str()) {
                    // Check if parent directory has a file with the same stem
                    if let Some(grandparent) = parent.parent()
                        && let Some(parent_stems) = file_stems_by_dir.get(grandparent)
                            && parent_stems.contains(stem) {
                                // Skip this file - it's a duplicate in a subdirectory
                                continue;
                            }
                }

            if let Ok(metadata) = fs::metadata(&path_buf) {
                // Try to get real audio metadata, fall back to estimates if it fails
                let (duration, bitrate, codec, is_lossy) = get_audio_metadata(&path_buf)
                    .unwrap_or_else(|_| {
                        // Fallback: estimate based on file size (assume 320kbps MP3)
                        let estimated_duration = (metadata.len() * 8) as f64 / (320.0 * 1000.0);
                        let ext = path_buf
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("mp3")
                            .to_lowercase();
                        let is_lossy_ext =
                            matches!(ext.as_str(), "mp3" | "aac" | "ogg" | "opus" | "m4a");
                        (estimated_duration, 320, ext, is_lossy_ext)
                    });

                files.push(AudioFileInfo {
                    path: path_buf,
                    duration,
                    bitrate,
                    size: metadata.len(),
                    codec,
                    is_lossy,
                });
            }
        }
    }

    // Sort files by path for consistent ordering
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(files)
}

/// Scan a single audio file and return its metadata
///
/// This is used when adding individual files to a mixtape.
pub fn scan_audio_file(path: &Path) -> Result<AudioFileInfo, String> {
    if !path.is_file() {
        return Err(format!("Path is not a file: {}", path.display()));
    }

    if !is_audio_file(path) {
        return Err(format!("Not an audio file: {}", path.display()));
    }

    let metadata = fs::metadata(path)
        .map_err(|e| format!("Failed to get file metadata: {}", e))?;

    let (duration, bitrate, codec, is_lossy) = get_audio_metadata(path)
        .unwrap_or_else(|_| {
            // Fallback: estimate based on file size (assume 320kbps MP3)
            let estimated_duration = (metadata.len() * 8) as f64 / (320.0 * 1000.0);
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("mp3")
                .to_lowercase();
            let is_lossy_ext = matches!(ext.as_str(), "mp3" | "aac" | "ogg" | "opus" | "m4a");
            (estimated_duration, 320, ext, is_lossy_ext)
        });

    Ok(AudioFileInfo {
        path: path.to_path_buf(),
        duration,
        bitrate,
        size: metadata.len(),
        codec,
        is_lossy,
    })
}

/// Calculate the total duration of a list of audio files
#[allow(dead_code)]
pub fn total_duration(files: &[AudioFileInfo]) -> f64 {
    files.iter().map(|f| f.duration).sum()
}

/// Calculate the total size of a list of audio files
#[allow(dead_code)]
pub fn total_size(files: &[AudioFileInfo]) -> u64 {
    files.iter().map(|f| f.size).sum()
}

/// Format duration as "Xm Ys"
pub fn format_duration(seconds: f64) -> String {
    let total_secs = seconds.round() as u64;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{}m {}s", mins, secs)
}

/// Format size in human-readable form (KB, MB, GB)
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(0.0), "0m 0s");
        assert_eq!(format_duration(30.0), "0m 30s");
        assert_eq!(format_duration(60.0), "1m 0s");
        assert_eq!(format_duration(90.0), "1m 30s");
        assert_eq!(format_duration(3661.0), "61m 1s");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 bytes");
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1536), "1.50 KB");
        assert_eq!(format_size(1048576), "1.00 MB");
        assert_eq!(format_size(1073741824), "1.00 GB");
    }

    #[test]
    fn test_scan_nonexistent_directory() {
        let result = scan_music_folder(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_scan_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let result = scan_music_folder(temp_dir.path()).unwrap();
        assert_eq!(result.file_count, 0);
        assert_eq!(result.total_size, 0);
        assert!(result.album_art.is_none());
    }

    #[test]
    fn test_scan_directory_with_non_audio_files() {
        let temp_dir = TempDir::new().unwrap();

        // Create some non-audio files
        let mut txt_file = File::create(temp_dir.path().join("readme.txt")).unwrap();
        writeln!(txt_file, "This is a readme file").unwrap();

        let mut json_file = File::create(temp_dir.path().join("data.json")).unwrap();
        writeln!(json_file, "{{}}").unwrap();

        let result = scan_music_folder(temp_dir.path()).unwrap();
        assert_eq!(result.file_count, 0); // No audio files
    }

    #[test]
    fn test_total_duration() {
        let files = vec![
            AudioFileInfo {
                path: PathBuf::from("/test/1.mp3"),
                duration: 180.0,
                bitrate: 320,
                size: 7200000,
                codec: "mp3".to_string(),
                is_lossy: true,
            },
            AudioFileInfo {
                path: PathBuf::from("/test/2.mp3"),
                duration: 240.0,
                bitrate: 320,
                size: 9600000,
                codec: "mp3".to_string(),
                is_lossy: true,
            },
        ];
        assert_eq!(total_duration(&files), 420.0);
    }

    #[test]
    fn test_total_size() {
        let files = vec![
            AudioFileInfo {
                path: PathBuf::from("/test/1.mp3"),
                duration: 180.0,
                bitrate: 320,
                size: 7200000,
                codec: "mp3".to_string(),
                is_lossy: true,
            },
            AudioFileInfo {
                path: PathBuf::from("/test/2.mp3"),
                duration: 240.0,
                bitrate: 320,
                size: 9600000,
                codec: "mp3".to_string(),
                is_lossy: true,
            },
        ];
        assert_eq!(total_size(&files), 16800000);
    }
}
