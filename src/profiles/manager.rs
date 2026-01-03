//! Profile manager - handles profile creation, saving, and loading
//!
//! This module extracts profile management logic from the folder list component.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::storage::{add_to_recent_profiles, is_bundle, load_profile, save_profile, validate_conversion_state};
use super::types::{BurnProfile, BurnSettings, ConversionStateValidation, SavedFolderState};
use crate::burning::IsoState;
use crate::conversion::OutputManager;
use crate::core::{FolderConversionStatus, MusicFolder};

/// Setup info for loading a profile asynchronously
///
/// This contains all the metadata needed to restore a profile without blocking
/// on folder scanning. The actual folder scanning happens in a background thread.
#[derive(Debug, Clone)]
pub struct ProfileLoadSetup {
    /// Profile name for display
    #[allow(dead_code)]
    pub profile_name: String,
    /// Ordered list of folder paths to scan
    pub folder_paths: Vec<PathBuf>,
    /// Validation result for saved conversion state
    pub validation: ConversionStateValidation,
    /// Saved folder states keyed by path string (for restoration)
    pub folder_states: HashMap<String, SavedFolderState>,
    /// ISO path if saved in profile
    pub iso_path: Option<PathBuf>,
    /// Custom volume label if saved in profile
    pub volume_label: Option<String>,
    /// Bundle path if this is a bundle format profile (v2.0+)
    /// None for legacy single-file profiles
    pub bundle_path: Option<PathBuf>,
}

/// Prepare to load a profile (fast, does not scan folders)
///
/// This reads and validates the profile, returning setup info needed for async loading.
/// The actual folder scanning should be done in a background thread.
pub fn prepare_profile_load(path: &Path) -> Result<ProfileLoadSetup, String> {
    let profile = load_profile(path)?;

    // Determine if this is a bundle format
    let bundle_path = if is_bundle(path) {
        Some(path.to_path_buf())
    } else {
        None
    };

    // Validate conversion state, passing bundle path if applicable
    let validation = validate_conversion_state(&profile, bundle_path.as_deref());

    println!("Loading profile: {} (bundle: {})", profile.profile_name, bundle_path.is_some());
    println!("  Valid folders: {:?}", validation.valid_folders);
    println!("  Invalid folders: {:?}", validation.invalid_folders);
    println!("  ISO valid: {}", validation.iso_valid);

    let folder_paths: Vec<PathBuf> = profile.folders.iter().map(PathBuf::from).collect();

    let folder_states = profile.folder_states.clone().unwrap_or_default();

    let iso_path = if validation.iso_valid {
        profile.iso_path.as_ref().map(PathBuf::from)
    } else {
        None
    };

    // Update recent profiles
    let _ = add_to_recent_profiles(&path.to_string_lossy());

    Ok(ProfileLoadSetup {
        profile_name: profile.profile_name,
        folder_paths,
        validation,
        folder_states,
        iso_path,
        volume_label: profile.volume_label,
        bundle_path,
    })
}

/// Create a BurnProfile from the current folder list state
///
/// This captures the current folder list and conversion state,
/// allowing the profile to be saved and later restored.
///
/// If `for_bundle` is true, uses v2.0 format with relative paths for output_dir.
/// If `for_bundle` is false, uses v1.x format with absolute paths (legacy).
pub fn create_profile(
    profile_name: String,
    folders: &[MusicFolder],
    output_manager: Option<&OutputManager>,
    iso_state: Option<&IsoState>,
    volume_label: Option<String>,
    for_bundle: bool,
) -> BurnProfile {
    let settings = BurnSettings {
        target_bitrate: "auto".to_string(),
        no_lossy_conversions: false,
        embed_album_art: true,
    };

    let folder_paths: Vec<String> = folders
        .iter()
        .map(|f| f.path.to_string_lossy().to_string())
        .collect();

    let mut profile = BurnProfile::new(profile_name, folder_paths, settings);
    profile.volume_label = volume_label;

    // Add conversion state if we have it
    if let Some(output_manager) = output_manager {
        let session_id = output_manager.session_id().to_string();

        // Build folder states map
        let mut folder_states = HashMap::new();
        for folder in folders {
            if let FolderConversionStatus::Converted {
                output_dir,
                lossless_bitrate,
                output_size,
                ..
            } = &folder.conversion_status
            {
                // Get source folder mtime
                let source_mtime = std::fs::metadata(&folder.path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                // Use relative path for bundle format, absolute for legacy
                let output_dir_str = if for_bundle {
                    output_manager.get_relative_output_path(&folder.id)
                } else {
                    output_dir.to_string_lossy().to_string()
                };

                let saved_state = SavedFolderState::new(
                    folder.id.0.clone(),
                    output_dir_str,
                    *lossless_bitrate,
                    *output_size,
                    source_mtime,
                    folder.file_count as usize,
                );
                folder_states.insert(folder.path.to_string_lossy().to_string(), saved_state);
            }
        }

        // Get ISO info if available (ISOs always use absolute paths - they're in /tmp)
        let (iso_path, iso_hash) = match iso_state {
            Some(iso) => (
                Some(iso.path.to_string_lossy().to_string()),
                Some(iso.folder_hash.clone()),
            ),
            None => (None, None),
        };

        if !folder_states.is_empty() {
            profile.set_conversion_state(session_id, folder_states, iso_path, iso_hash);
        }
    }

    // Set version based on format (AFTER set_conversion_state which sets it to 1.1)
    if for_bundle {
        profile.version = "2.0".to_string();
    }

    profile
}

/// Save a profile to the specified path
///
/// If `for_bundle` is true, saves as v2.0 bundle format with relative paths.
/// If `for_bundle` is false, saves as legacy v1.x format with absolute paths.
///
/// Returns Ok(()) on success, or an error message on failure.
pub fn save_profile_to_path(
    path: &Path,
    profile_name: String,
    folders: &[MusicFolder],
    output_manager: Option<&OutputManager>,
    iso_state: Option<&IsoState>,
    volume_label: Option<String>,
    for_bundle: bool,
) -> Result<(), String> {
    let profile = create_profile(profile_name, folders, output_manager, iso_state, volume_label, for_bundle);
    save_profile(&profile, path)?;
    add_to_recent_profiles(&path.to_string_lossy())?;
    println!("Profile saved to: {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_profile_empty_folders() {
        let profile = create_profile("Test".to_string(), &[], None, None, None, false);
        assert_eq!(profile.profile_name, "Test");
        assert!(profile.folders.is_empty());
    }

    #[test]
    fn test_create_profile_with_folders() {
        let folders = vec![MusicFolder::new_for_test("/test/album")];

        let profile = create_profile("My Album".to_string(), &folders, None, None, None, false);
        assert_eq!(profile.profile_name, "My Album");
        assert_eq!(profile.folders.len(), 1);
        assert_eq!(profile.folders[0], "/test/album");
    }

    #[test]
    fn test_create_profile_with_volume_label() {
        let folders = vec![MusicFolder::new_for_test("/test/album")];

        let profile = create_profile(
            "My Album".to_string(),
            &folders,
            None,
            None,
            Some("My CD".to_string()),
            false,
        );
        assert_eq!(profile.volume_label, Some("My CD".to_string()));
    }

    #[test]
    fn test_create_profile_for_bundle() {
        let folders = vec![MusicFolder::new_for_test("/test/album")];

        let profile = create_profile("Bundle Test".to_string(), &folders, None, None, None, true);
        assert_eq!(profile.version, "2.0");
    }

    #[test]
    fn test_create_profile_legacy() {
        let folders = vec![MusicFolder::new_for_test("/test/album")];

        let profile = create_profile("Legacy Test".to_string(), &folders, None, None, None, false);
        assert_eq!(profile.version, "1.0");
    }

    #[test]
    fn test_save_and_load_profile() {
        let temp_dir = TempDir::new().unwrap();
        let profile_path = temp_dir.path().join("test.mp3cd");

        let folders = vec![MusicFolder::new_for_test("/test/album")];

        // Save as legacy (non-bundle)
        let result = save_profile_to_path(
            &profile_path,
            "Test Profile".to_string(),
            &folders,
            None,
            None,
            Some("Test CD".to_string()),
            false, // legacy format
        );
        assert!(result.is_ok());
        assert!(profile_path.exists());
    }

    #[test]
    fn test_save_bundle_profile() {
        let temp_dir = TempDir::new().unwrap();
        let profile_path = temp_dir.path().join("test.mp3cd");

        let folders = vec![MusicFolder::new_for_test("/test/album")];

        // Save as bundle
        let result = save_profile_to_path(
            &profile_path,
            "Bundle Profile".to_string(),
            &folders,
            None,
            None,
            Some("Test CD".to_string()),
            true, // bundle format
        );
        assert!(result.is_ok());
        // Bundle creates a directory
        assert!(profile_path.is_dir());
        assert!(profile_path.join("profile.json").exists());
        assert!(profile_path.join("converted").is_dir());
    }
}
