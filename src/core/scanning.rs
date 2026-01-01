//! Folder scanning and audio file discovery
//!
//! This module provides functions for scanning music folders, discovering
//! audio files, and collecting metadata about them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::audio::{get_album_art, get_audio_metadata, is_audio_file};

/// Represents metadata about a music folder
#[derive(Debug, Clone)]
pub struct MusicFolder {
    pub path: PathBuf,
    pub file_count: u32,
    pub total_size: u64,
    pub total_duration: f64,
    pub album_art: Option<String>,
    /// Cached audio file info for bitrate calculation
    pub audio_files: Vec<AudioFileInfo>,
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

    // Extract album art from the first audio file
    let album_art = audio_files.first().and_then(|f| get_album_art(&f.path));

    Ok(MusicFolder {
        path: path.to_path_buf(),
        file_count,
        total_size,
        total_duration,
        album_art,
        audio_files,
    })
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
        if path_buf.is_file() && is_audio_file(&path_buf) {
            if let Some(parent) = path_buf.parent() {
                if let Some(stem) = path_buf.file_stem().and_then(|s| s.to_str()) {
                    file_stems_by_dir
                        .entry(parent.to_path_buf())
                        .or_default()
                        .insert(stem.to_string());
                }
            }
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
            if let Some(parent) = path_buf.parent() {
                if let Some(stem) = path_buf.file_stem().and_then(|s| s.to_str()) {
                    // Check if parent directory has a file with the same stem
                    if let Some(grandparent) = parent.parent() {
                        if let Some(parent_stems) = file_stems_by_dir.get(grandparent) {
                            if parent_stems.contains(stem) {
                                // Skip this file - it's a duplicate in a subdirectory
                                continue;
                            }
                        }
                    }
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
                        let is_lossy_ext = matches!(ext.as_str(), "mp3" | "aac" | "ogg" | "opus" | "m4a");
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
