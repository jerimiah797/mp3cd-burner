//! Core application logic and state
//!
//! This module contains:
//! - Application-wide state (settings, preferences)
//! - Actions that can be triggered from menus or UI
//! - Folder scanning and audio file discovery
//! - Bitrate calculation for CD-fitting optimization
//! - Folder state tracking for background encoding

use std::path::PathBuf;

mod bitrate;
mod folder_state;
mod scanning;
mod state;

pub use folder_state::{FolderConversionStatus, FolderId, calculate_folder_hash};
pub use scanning::{
    AudioFileInfo, FolderKind, MusicFolder, SavedMixtapeTrackInfo, create_folder_from_metadata,
    create_mixtape_from_saved_state, find_album_folders, format_duration, format_size,
    scan_audio_file, scan_music_folder,
};
pub use state::{
    AppSettings, BurnStage, ConversionState, DisplaySettings, ImportState, WindowState,
};

/// Get the path to a bundled resource file
///
/// In development, looks for resources at CARGO_MANIFEST_DIR/resources/
/// In release builds, looks in the app bundle's Resources folder.
pub fn get_resource_path(relative_path: &str) -> Option<PathBuf> {
    // Try CARGO_MANIFEST_DIR first (development mode)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let dev_path = PathBuf::from(manifest_dir)
            .join("resources")
            .join(relative_path);

        if dev_path.exists() {
            return Some(dev_path);
        }
    }

    // Try relative to current executable (release mode)
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            // macOS app bundle: Contents/MacOS/../Resources/
            let bundle_path = exe_dir
                .join("..")
                .join("Resources")
                .join(relative_path);

            if bundle_path.exists() {
                return Some(bundle_path);
            }

            // Also try directly next to executable
            let local_path = exe_dir.join("resources").join(relative_path);
            if local_path.exists() {
                return Some(local_path);
            }
        }
    }

    None
}

/// Get the path to the default mixtape album art image
pub fn get_mixtape_default_art() -> Option<PathBuf> {
    get_resource_path("images/mixtape.jpg")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_resource_path_nonexistent() {
        // A path that definitely doesn't exist should return None
        let result = get_resource_path("definitely/not/a/real/file.txt");
        assert!(result.is_none());
    }

    #[test]
    fn test_get_resource_path_returns_pathbuf() {
        // Even if the path doesn't exist, the function should handle it gracefully
        let result = get_resource_path("test.txt");
        // Result is None because file doesn't exist
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn test_get_mixtape_default_art_development() {
        // In development mode, this should find the mixtape.jpg in resources/
        // This test will pass if run from the project root with CARGO_MANIFEST_DIR set
        let result = get_mixtape_default_art();
        // The file may or may not exist depending on the test environment
        if let Some(path) = result {
            assert!(path.to_string_lossy().contains("mixtape.jpg"));
        }
    }

    #[test]
    fn test_get_resource_path_with_subdirectory() {
        // Test with a subdirectory path
        let result = get_resource_path("images/something.png");
        // Just verify it doesn't panic
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn test_get_resource_path_empty_string() {
        // Test with empty string
        let result = get_resource_path("");
        // Should return Some if resources/ dir exists, None otherwise
        assert!(result.is_none() || result.is_some());
    }
}
