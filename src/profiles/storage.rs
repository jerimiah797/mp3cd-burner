//! Profile storage for saving/loading folder configurations
//!
//! Supports two formats:
//! - **Bundle format (v2.0+)**: `.mp3cd` directory containing `profile.json` and `converted/` folder
//! - **Legacy format (v1.x)**: Single `.mp3cd` JSON file (deprecated)
#![allow(dead_code)]

use super::types::{BurnProfile, ConversionStateValidation};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const RECENT_PROFILES_FILE: &str = "recent_profiles.json";
const MAX_RECENT_PROFILES: usize = 10;

/// Check if a path is a bundle (directory with .mp3cd extension)
pub fn is_bundle(path: &Path) -> bool {
    path.is_dir() && path.extension().is_some_and(|ext| ext == "mp3cd")
}

/// Get the profile.json path inside a bundle
pub fn get_profile_json_path(bundle_path: &Path) -> PathBuf {
    bundle_path.join("profile.json")
}

/// Get the converted/ directory path inside a bundle
pub fn get_converted_dir(bundle_path: &Path) -> PathBuf {
    bundle_path.join("converted")
}

/// Save a burn profile to a file or bundle
///
/// If the path ends with `.mp3cd`:
/// - Creates a bundle directory structure
/// - Writes `profile.json` inside the bundle
pub fn save_profile(profile: &BurnProfile, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(profile)
        .map_err(|e| format!("Failed to serialize profile: {}", e))?;

    // Check if this should be a bundle (path ends with .mp3cd and we're saving v2.0+)
    let is_bundle_path = path.extension().is_some_and(|ext| ext == "mp3cd");

    if is_bundle_path && profile.version.starts_with("2.") {
        // If path exists as a file (old metadata-only profile), remove it first
        // so we can create a bundle directory in its place
        if path.is_file() {
            fs::remove_file(path)
                .map_err(|e| format!("Failed to remove existing profile file: {}", e))?;
            println!("Removed existing metadata-only profile to create bundle");
        }

        // Create bundle directory structure
        fs::create_dir_all(path)
            .map_err(|e| format!("Failed to create bundle directory: {}", e))?;

        // Create converted/ subdirectory
        let converted_dir = get_converted_dir(path);
        fs::create_dir_all(&converted_dir)
            .map_err(|e| format!("Failed to create converted directory: {}", e))?;

        // Write profile.json inside bundle
        let profile_json_path = get_profile_json_path(path);
        fs::write(&profile_json_path, json)
            .map_err(|e| format!("Failed to write profile.json: {}", e))?;

        println!("Saved bundle profile to: {:?}", path);
    } else {
        // Legacy: write directly to path
        fs::write(path, json).map_err(|e| format!("Failed to write profile file: {}", e))?;
    }

    Ok(())
}

/// Load a burn profile from a file or bundle
///
/// Automatically detects:
/// - Bundle format: directory containing `profile.json`
/// - Legacy format: single JSON file
pub fn load_profile(path: &Path) -> Result<BurnProfile, String> {
    let profile_path = if is_bundle(path) {
        // Bundle format: read profile.json from inside
        get_profile_json_path(path)
    } else {
        // Legacy format: read directly
        path.to_path_buf()
    };

    let contents = fs::read_to_string(&profile_path)
        .map_err(|e| format!("Failed to read profile file: {}", e))?;

    let profile: BurnProfile = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse profile file: {}", e))?;

    Ok(profile)
}

/// Validate a profile's saved conversion state
///
/// Checks if:
/// - Output directories exist for each folder (in bundle or temp)
/// - Source folders haven't been modified
/// - ISO file exists (if saved)
///
/// For bundle format: `bundle_path` should be the path to the `.mp3cd` bundle.
/// For legacy format: `bundle_path` should be `None`.
pub fn validate_conversion_state(
    profile: &BurnProfile,
    bundle_path: Option<&Path>,
) -> ConversionStateValidation {
    let mut validation = ConversionStateValidation {
        session_exists: false,
        valid_folders: Vec::new(),
        invalid_folders: Vec::new(),
        iso_valid: false,
    };

    // Check if profile has conversion state
    let folder_states = match &profile.folder_states {
        Some(states) => states,
        None => {
            // No conversion state - all folders need encoding
            validation.invalid_folders = profile.folders.clone();
            return validation;
        }
    };

    // Determine base directory for resolving output paths
    let base_dir: Option<PathBuf> = if let Some(bundle) = bundle_path {
        // Bundle format: output_dir is relative to bundle (e.g., "converted/abc123...")
        Some(bundle.to_path_buf())
    } else if let Some(session_id) = &profile.session_id {
        // Legacy format: output_dir is relative to session dir
        let session_dir = std::env::temp_dir().join("mp3cd_output").join(session_id);
        if session_dir.exists() {
            validation.session_exists = true;
            Some(session_dir)
        } else {
            // Session directory gone - all folders invalid
            validation.invalid_folders = profile.folders.clone();
            return validation;
        }
    } else {
        // No session_id and no bundle - can't validate
        validation.invalid_folders = profile.folders.clone();
        return validation;
    };

    // For bundles, session_exists means the bundle exists (which it does if we got here)
    if bundle_path.is_some() {
        validation.session_exists = true;
    }

    // Check each folder
    for folder_path in &profile.folders {
        if let Some(saved_state) = folder_states.get(folder_path) {
            // Resolve output directory path
            let output_dir = if let Some(base) = &base_dir {
                base.join(&saved_state.output_dir)
            } else {
                PathBuf::from(&saved_state.output_dir)
            };

            if !output_dir.exists() {
                validation.invalid_folders.push(folder_path.clone());
                continue;
            }

            // Check if source folder has been modified
            let source_path = Path::new(folder_path);
            if let Ok(metadata) = fs::metadata(source_path) {
                let current_mtime = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                if saved_state.source_modified(current_mtime) {
                    // Source changed - needs re-encoding
                    validation.invalid_folders.push(folder_path.clone());
                } else {
                    // Still valid
                    validation.valid_folders.push(folder_path.clone());
                }
            } else {
                // Can't access source - assume invalid
                validation.invalid_folders.push(folder_path.clone());
            }
        } else {
            // No saved state for this folder
            validation.invalid_folders.push(folder_path.clone());
        }
    }

    // Check ISO validity (ISOs are always in temp, not in bundle)
    if let Some(iso_path) = &profile.iso_path {
        let iso_exists = Path::new(iso_path).exists();
        let hash_matches =
            profile.iso_folder_hash.is_some() && validation.invalid_folders.is_empty();
        validation.iso_valid = iso_exists && hash_matches;
    }

    validation
}

/// Get the path to the app's data directory for storing recent profiles
fn get_app_data_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Could not determine home directory".to_string())?;

    let app_data = home.join(".mp3cd-burner");

    // Create directory if it doesn't exist
    if !app_data.exists() {
        fs::create_dir_all(&app_data)
            .map_err(|e| format!("Failed to create app data directory: {}", e))?;
    }

    Ok(app_data)
}

/// Get the path to the recent profiles file
fn get_recent_profiles_path() -> Result<PathBuf, String> {
    Ok(get_app_data_dir()?.join(RECENT_PROFILES_FILE))
}

/// Load the list of recent profile paths
pub fn load_recent_profiles() -> Result<Vec<String>, String> {
    let path = get_recent_profiles_path()?;

    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read recent profiles: {}", e))?;

    let recent: Vec<String> = serde_json::from_str(&contents).unwrap_or_else(|_| Vec::new());

    // Filter out paths that no longer exist
    let existing: Vec<String> = recent
        .into_iter()
        .filter(|p| Path::new(p).exists())
        .collect();

    Ok(existing)
}

/// Add a profile path to the recent profiles list
pub fn add_to_recent_profiles(profile_path: &str) -> Result<(), String> {
    let mut recent = load_recent_profiles()?;

    // Remove if already in list
    recent.retain(|p| p != profile_path);

    // Add to front
    recent.insert(0, profile_path.to_string());

    // Limit to MAX_RECENT_PROFILES
    if recent.len() > MAX_RECENT_PROFILES {
        recent.truncate(MAX_RECENT_PROFILES);
    }

    // Save updated list
    let path = get_recent_profiles_path()?;
    let json = serde_json::to_string_pretty(&recent)
        .map_err(|e| format!("Failed to serialize recent profiles: {}", e))?;

    fs::write(&path, json).map_err(|e| format!("Failed to write recent profiles: {}", e))?;

    Ok(())
}

/// Remove a profile path from the recent profiles list
pub fn remove_from_recent_profiles(profile_path: &str) -> Result<(), String> {
    let mut recent = load_recent_profiles()?;

    recent.retain(|p| p != profile_path);

    let path = get_recent_profiles_path()?;
    let json = serde_json::to_string_pretty(&recent)
        .map_err(|e| format!("Failed to serialize recent profiles: {}", e))?;

    fs::write(&path, json).map_err(|e| format!("Failed to write recent profiles: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::types::BurnSettings;
    use tempfile::TempDir;

    #[test]
    fn test_save_and_load_profile() {
        let temp_dir = TempDir::new().unwrap();
        let profile_path = temp_dir.path().join("test.mp3cd");

        let settings = BurnSettings {
            target_bitrate: "auto".to_string(),
            no_lossy_conversions: false,
            embed_album_art: true,
        };

        let profile = BurnProfile::new(
            "Test".to_string(),
            vec!["/music/test".to_string()],
            settings,
        );

        // Save
        save_profile(&profile, &profile_path).unwrap();
        assert!(profile_path.exists());

        // Load
        let loaded = load_profile(&profile_path).unwrap();
        assert_eq!(loaded.profile_name, "Test");
        assert_eq!(loaded.folders[0], "/music/test");
    }
}
