use super::types::BurnProfile;
use std::fs;
use std::path::{Path, PathBuf};

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
        let profile_path = temp_dir.path().join("test.burn");

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
