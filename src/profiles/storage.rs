//! Profile storage for saving/loading folder configurations
//! (Future feature)
#![allow(dead_code)]

use super::types::{BurnProfile, ConversionStateValidation};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const RECENT_PROFILES_FILE: &str = "recent_profiles.json";
const MAX_RECENT_PROFILES: usize = 10;

/// Save a burn profile to a file
pub fn save_profile(profile: &BurnProfile, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(profile)
        .map_err(|e| format!("Failed to serialize profile: {}", e))?;

    fs::write(path, json)
        .map_err(|e| format!("Failed to write profile file: {}", e))?;

    Ok(())
}

/// Load a burn profile from a file
pub fn load_profile(path: &Path) -> Result<BurnProfile, String> {
    let contents = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read profile file: {}", e))?;

    let profile: BurnProfile = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse profile file: {}", e))?;

    Ok(profile)
}

/// Validate a profile's saved conversion state
///
/// Checks if:
/// - Session directory exists
/// - Output directories exist for each folder
/// - Source folders haven't been modified
/// - ISO file exists (if saved)
pub fn validate_conversion_state(profile: &BurnProfile) -> ConversionStateValidation {
    let mut validation = ConversionStateValidation {
        session_exists: false,
        valid_folders: Vec::new(),
        invalid_folders: Vec::new(),
        iso_valid: false,
    };

    // Check if profile has conversion state
    let (session_id, folder_states) = match (&profile.session_id, &profile.folder_states) {
        (Some(sid), Some(states)) => (sid, states),
        _ => return validation, // No conversion state saved
    };

    // Check session directory
    let session_dir = std::env::temp_dir()
        .join("mp3cd_output")
        .join(session_id);

    if !session_dir.exists() {
        // Session directory gone - all folders invalid
        validation.invalid_folders = profile.folders.clone();
        return validation;
    }

    validation.session_exists = true;

    // Check each folder
    for folder_path in &profile.folders {
        if let Some(saved_state) = folder_states.get(folder_path) {
            // Check if output directory exists
            let output_dir = session_dir.join(&saved_state.output_dir);
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

    // Check ISO validity
    if let Some(iso_path) = &profile.iso_path {
        let iso_exists = Path::new(iso_path).exists();
        let hash_matches = profile.iso_folder_hash.is_some()
            && validation.invalid_folders.is_empty();
        validation.iso_valid = iso_exists && hash_matches;
    }

    validation
}

/// Get the path to the app's data directory for storing recent profiles
fn get_app_data_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir()
        .ok_or_else(|| "Could not determine home directory".to_string())?;

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

    let contents = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read recent profiles: {}", e))?;

    let recent: Vec<String> = serde_json::from_str(&contents)
        .unwrap_or_else(|_| Vec::new());

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

    fs::write(&path, json)
        .map_err(|e| format!("Failed to write recent profiles: {}", e))?;

    Ok(())
}

/// Remove a profile path from the recent profiles list
pub fn remove_from_recent_profiles(profile_path: &str) -> Result<(), String> {
    let mut recent = load_recent_profiles()?;

    recent.retain(|p| p != profile_path);

    let path = get_recent_profiles_path()?;
    let json = serde_json::to_string_pretty(&recent)
        .map_err(|e| format!("Failed to serialize recent profiles: {}", e))?;

    fs::write(&path, json)
        .map_err(|e| format!("Failed to write recent profiles: {}", e))?;

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
