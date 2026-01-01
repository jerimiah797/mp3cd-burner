//! Profile types for saving/loading folder configurations
//! (Future feature)
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Represents a burn profile - a saved configuration for burning a CD
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurnProfile {
    /// Version of the profile format (for future compatibility)
    pub version: String,

    /// User-friendly name for this profile
    pub profile_name: String,

    /// When this profile was created (ISO 8601 format)
    pub created: String,

    /// When this profile was last modified (ISO 8601 format)
    pub modified: String,

    /// Ordered list of folder paths to burn
    pub folders: Vec<String>,

    /// Burn settings
    pub settings: BurnSettings,

    /// Custom volume label (if user has edited it)
    /// If None, auto-generate from folders
    pub volume_label: Option<String>,
}

/// Settings for burning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurnSettings {
    /// Target bitrate ("auto" or specific number like "320")
    pub target_bitrate: String,

    /// Whether to avoid lossy-to-lossy conversions
    pub no_lossy_conversions: bool,

    /// Whether to embed album art in output files
    pub embed_album_art: bool,
}

impl BurnProfile {
    /// Create a new burn profile with current timestamp
    pub fn new(profile_name: String, folders: Vec<String>, settings: BurnSettings) -> Self {
        let now = chrono::Utc::now().to_rfc3339();

        Self {
            version: "1.0".to_string(),
            profile_name,
            created: now.clone(),
            modified: now,
            folders,
            settings,
            volume_label: None,
        }
    }

    /// Update the modified timestamp
    pub fn touch(&mut self) {
        self.modified = chrono::Utc::now().to_rfc3339();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_burn_profile() {
        let settings = BurnSettings {
            target_bitrate: "auto".to_string(),
            no_lossy_conversions: false,
            embed_album_art: true,
        };

        let profile = BurnProfile::new(
            "Test Profile".to_string(),
            vec!["/music/album1".to_string(), "/music/album2".to_string()],
            settings,
        );

        assert_eq!(profile.version, "1.0");
        assert_eq!(profile.profile_name, "Test Profile");
        assert_eq!(profile.folders.len(), 2);
        assert!(profile.volume_label.is_none());
    }

    #[test]
    fn test_profile_serialization() {
        let settings = BurnSettings {
            target_bitrate: "320".to_string(),
            no_lossy_conversions: true,
            embed_album_art: false,
        };

        let profile = BurnProfile::new(
            "Radiohead".to_string(),
            vec!["/music/radiohead/ok_computer".to_string()],
            settings,
        );

        let json = serde_json::to_string(&profile).unwrap();
        let deserialized: BurnProfile = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.profile_name, "Radiohead");
        assert_eq!(deserialized.folders[0], "/music/radiohead/ok_computer");
        assert_eq!(deserialized.settings.target_bitrate, "320");
    }
}
