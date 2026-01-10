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
            log::debug!("Removed existing metadata-only profile to create bundle");
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

        log::debug!("Saved bundle profile to: {:?}", path);
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

    fn make_test_settings() -> BurnSettings {
        BurnSettings {
            target_bitrate: "auto".to_string(),
            no_lossy_conversions: false,
            embed_album_art: true,
        }
    }

    #[test]
    fn test_save_and_load_profile() {
        let temp_dir = TempDir::new().unwrap();
        let profile_path = temp_dir.path().join("test.mp3cd");

        let profile = BurnProfile::new(
            "Test".to_string(),
            vec!["/music/test".to_string()],
            make_test_settings(),
        );

        // Save
        save_profile(&profile, &profile_path).unwrap();
        assert!(profile_path.exists());

        // Load
        let loaded = load_profile(&profile_path).unwrap();
        assert_eq!(loaded.profile_name, "Test");
        assert_eq!(loaded.folders[0], "/music/test");
    }

    #[test]
    fn test_is_bundle_directory() {
        let temp_dir = TempDir::new().unwrap();

        // Regular directory - not a bundle
        let regular_dir = temp_dir.path().join("regular");
        fs::create_dir(&regular_dir).unwrap();
        assert!(!is_bundle(&regular_dir));

        // Directory with .mp3cd extension - is a bundle
        let bundle_dir = temp_dir.path().join("test.mp3cd");
        fs::create_dir(&bundle_dir).unwrap();
        assert!(is_bundle(&bundle_dir));
    }

    #[test]
    fn test_is_bundle_file() {
        let temp_dir = TempDir::new().unwrap();

        // File with .mp3cd extension - not a bundle (bundles are directories)
        let profile_file = temp_dir.path().join("test.mp3cd");
        fs::write(&profile_file, "{}").unwrap();
        assert!(!is_bundle(&profile_file));
    }

    #[test]
    fn test_get_profile_json_path() {
        let bundle_path = Path::new("/path/to/test.mp3cd");
        let json_path = get_profile_json_path(bundle_path);
        assert_eq!(json_path, PathBuf::from("/path/to/test.mp3cd/profile.json"));
    }

    #[test]
    fn test_get_converted_dir() {
        let bundle_path = Path::new("/path/to/test.mp3cd");
        let converted = get_converted_dir(bundle_path);
        assert_eq!(converted, PathBuf::from("/path/to/test.mp3cd/converted"));
    }

    #[test]
    fn test_save_bundle_format() {
        let temp_dir = TempDir::new().unwrap();
        let bundle_path = temp_dir.path().join("bundle.mp3cd");

        let mut profile = BurnProfile::new(
            "Bundle Test".to_string(),
            vec!["/music/album".to_string()],
            make_test_settings(),
        );
        profile.version = "2.0".to_string();

        // Save as bundle
        save_profile(&profile, &bundle_path).unwrap();

        // Verify bundle structure
        assert!(bundle_path.is_dir());
        assert!(bundle_path.join("profile.json").exists());
        assert!(bundle_path.join("converted").is_dir());
    }

    #[test]
    fn test_load_bundle_format() {
        let temp_dir = TempDir::new().unwrap();
        let bundle_path = temp_dir.path().join("load.mp3cd");

        // Create bundle structure manually
        fs::create_dir(&bundle_path).unwrap();
        fs::create_dir(bundle_path.join("converted")).unwrap();

        let profile_json = r#"{
            "version": "2.0",
            "profile_name": "Loaded Bundle",
            "folders": ["/music/test"],
            "settings": {
                "target_bitrate": "auto",
                "no_lossy_conversions": false,
                "embed_album_art": true
            },
            "created": "2024-01-01T00:00:00Z",
            "modified": "2024-01-01T00:00:00Z"
        }"#;
        fs::write(bundle_path.join("profile.json"), profile_json).unwrap();

        // Load
        let loaded = load_profile(&bundle_path).unwrap();
        assert_eq!(loaded.profile_name, "Loaded Bundle");
        assert_eq!(loaded.version, "2.0");
    }

    #[test]
    fn test_load_profile_not_found() {
        let result = load_profile(Path::new("/nonexistent/path.mp3cd"));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_no_conversion_state() {
        let profile = BurnProfile::new(
            "Test".to_string(),
            vec!["/folder1".to_string(), "/folder2".to_string()],
            make_test_settings(),
        );

        let validation = validate_conversion_state(&profile, None);

        assert!(!validation.session_exists);
        assert!(validation.valid_folders.is_empty());
        assert_eq!(validation.invalid_folders.len(), 2);
        assert!(!validation.iso_valid);
    }

    #[test]
    fn test_validate_no_session_id() {
        let mut profile = BurnProfile::new(
            "Test".to_string(),
            vec!["/folder".to_string()],
            make_test_settings(),
        );
        // Add folder_states but no session_id
        profile.folder_states = Some(std::collections::HashMap::new());

        let validation = validate_conversion_state(&profile, None);

        assert!(!validation.session_exists);
        assert_eq!(validation.invalid_folders.len(), 1);
    }

    #[test]
    fn test_validate_bundle_format() {
        let temp_dir = TempDir::new().unwrap();
        let bundle_path = temp_dir.path().join("test.mp3cd");
        fs::create_dir(&bundle_path).unwrap();

        let mut profile = BurnProfile::new(
            "Test".to_string(),
            vec!["/folder".to_string()],
            make_test_settings(),
        );
        profile.folder_states = Some(std::collections::HashMap::new());

        let validation = validate_conversion_state(&profile, Some(&bundle_path));

        // Session exists because bundle exists
        assert!(validation.session_exists);
        // Folder is invalid because no saved state for it
        assert_eq!(validation.invalid_folders.len(), 1);
    }

    #[test]
    fn test_recent_profiles_add_and_load() {
        // This test modifies real app data, so we just verify the functions work
        let test_path = "/tmp/test_profile_12345.mp3cd";

        // Add to recent
        let result = add_to_recent_profiles(test_path);
        assert!(result.is_ok());

        // Load recent
        let recent = load_recent_profiles().unwrap();
        // Should contain our path (if it exists) or be filtered out
        // Since /tmp/test_profile_12345.mp3cd doesn't exist, it gets filtered

        // Clean up
        let _ = remove_from_recent_profiles(test_path);
        assert!(recent.is_empty() || !recent.contains(&test_path.to_string()));
    }

    #[test]
    fn test_recent_profiles_removes_duplicates() {
        let temp_dir = TempDir::new().unwrap();
        let test_path = temp_dir.path().join("dup_test.mp3cd");
        let test_path_str = test_path.to_string_lossy().to_string();

        // Create the file
        fs::write(&test_path, "{}").unwrap();

        // Add twice
        add_to_recent_profiles(&test_path_str).unwrap();
        add_to_recent_profiles(&test_path_str).unwrap();

        let recent = load_recent_profiles().unwrap();
        let count = recent.iter().filter(|p| *p == &test_path_str).count();
        assert!(count <= 1); // Should only appear once (or zero if filtered)

        // Clean up
        let _ = remove_from_recent_profiles(&test_path_str);
    }

    #[test]
    fn test_remove_from_recent_profiles() {
        let temp_dir = TempDir::new().unwrap();
        let test_path = temp_dir.path().join("remove_test.mp3cd");
        let test_path_str = test_path.to_string_lossy().to_string();

        // Create the file
        fs::write(&test_path, "{}").unwrap();

        // Add then remove
        add_to_recent_profiles(&test_path_str).unwrap();
        remove_from_recent_profiles(&test_path_str).unwrap();

        let recent = load_recent_profiles().unwrap();
        assert!(!recent.contains(&test_path_str));
    }
}
