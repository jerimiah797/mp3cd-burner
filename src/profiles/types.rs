//! Profile types for saving/loading folder configurations
//! (Future feature)
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

    /// Manual bitrate override (if user has set a custom bitrate)
    /// If None, use auto-calculated bitrate
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_bitrate_override: Option<u32>,

    // === Conversion State (v1.1+) ===
    /// Session ID for the output directory
    /// Output is stored in /tmp/mp3cd_output/{session_id}/
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Per-folder conversion state, keyed by folder path
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_states: Option<HashMap<String, SavedFolderState>>,

    /// Path to the ISO file (if one exists)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iso_path: Option<String>,

    /// Hash of folder IDs + order for ISO validity check
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iso_folder_hash: Option<String>,
}

/// Saved conversion state for a single folder
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFolderState {
    /// Folder ID (hash of path + mtime at time of encoding)
    pub folder_id: String,

    /// Output directory path (relative to session dir)
    pub output_dir: String,

    /// Bitrate used for lossless files (None if no lossless files)
    pub lossless_bitrate: Option<u32>,

    /// Total size of output files in bytes
    pub output_size: u64,

    /// Source folder modification time (for detecting changes)
    pub source_mtime: u64,

    /// Number of files encoded
    pub file_count: usize,
}

impl SavedFolderState {
    /// Create a new SavedFolderState
    pub fn new(
        folder_id: String,
        output_dir: String,
        lossless_bitrate: Option<u32>,
        output_size: u64,
        source_mtime: u64,
        file_count: usize,
    ) -> Self {
        Self {
            folder_id,
            output_dir,
            lossless_bitrate,
            output_size,
            source_mtime,
            file_count,
        }
    }

    /// Check if the source folder has been modified since encoding
    pub fn source_modified(&self, current_mtime: u64) -> bool {
        current_mtime != self.source_mtime
    }
}

/// Result of validating a profile's conversion state
#[derive(Debug, Clone)]
pub struct ConversionStateValidation {
    /// Whether the session directory exists
    pub session_exists: bool,

    /// Folders that are still valid (output exists, source unchanged)
    pub valid_folders: Vec<String>,

    /// Folders that need re-encoding (source changed or output missing)
    pub invalid_folders: Vec<String>,

    /// Whether the ISO is still valid
    pub iso_valid: bool,
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
            manual_bitrate_override: None,
            // Conversion state (v1.1+) - None for new profiles
            session_id: None,
            folder_states: None,
            iso_path: None,
            iso_folder_hash: None,
        }
    }

    /// Check if this profile has saved conversion state
    pub fn has_conversion_state(&self) -> bool {
        self.session_id.is_some() && self.folder_states.is_some()
    }

    /// Set the conversion state from current session
    pub fn set_conversion_state(
        &mut self,
        session_id: String,
        folder_states: HashMap<String, SavedFolderState>,
        iso_path: Option<String>,
        iso_folder_hash: Option<String>,
    ) {
        self.version = "1.1".to_string();
        self.session_id = Some(session_id);
        self.folder_states = Some(folder_states);
        self.iso_path = iso_path;
        self.iso_folder_hash = iso_folder_hash;
        self.touch();
    }

    /// Clear conversion state (e.g., when folders change significantly)
    pub fn clear_conversion_state(&mut self) {
        self.session_id = None;
        self.folder_states = None;
        self.iso_path = None;
        self.iso_folder_hash = None;
        self.touch();
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

    #[test]
    fn test_new_profile_has_no_conversion_state() {
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

        assert!(!profile.has_conversion_state());
        assert!(profile.session_id.is_none());
        assert!(profile.folder_states.is_none());
        assert!(profile.iso_path.is_none());
    }

    #[test]
    fn test_set_conversion_state() {
        let settings = BurnSettings {
            target_bitrate: "auto".to_string(),
            no_lossy_conversions: false,
            embed_album_art: true,
        };

        let mut profile = BurnProfile::new(
            "Test".to_string(),
            vec!["/music/album1".to_string()],
            settings,
        );

        let mut folder_states = HashMap::new();
        folder_states.insert(
            "/music/album1".to_string(),
            SavedFolderState::new(
                "abc123".to_string(),
                "abc123".to_string(),
                Some(320),
                50_000_000,
                1234567890,
                10,
            ),
        );

        profile.set_conversion_state(
            "session_123".to_string(),
            folder_states,
            Some("/tmp/test.iso".to_string()),
            Some("hash123".to_string()),
        );

        assert!(profile.has_conversion_state());
        assert_eq!(profile.version, "1.1");
        assert_eq!(profile.session_id, Some("session_123".to_string()));
        assert!(profile.folder_states.is_some());
        assert_eq!(profile.iso_path, Some("/tmp/test.iso".to_string()));
    }

    #[test]
    fn test_clear_conversion_state() {
        let settings = BurnSettings {
            target_bitrate: "auto".to_string(),
            no_lossy_conversions: false,
            embed_album_art: true,
        };

        let mut profile = BurnProfile::new(
            "Test".to_string(),
            vec!["/music/album1".to_string()],
            settings,
        );

        // Set some state
        profile.set_conversion_state(
            "session_123".to_string(),
            HashMap::new(),
            Some("/tmp/test.iso".to_string()),
            Some("hash123".to_string()),
        );

        assert!(profile.has_conversion_state());

        // Clear it
        profile.clear_conversion_state();

        assert!(!profile.has_conversion_state());
        assert!(profile.session_id.is_none());
        assert!(profile.folder_states.is_none());
        assert!(profile.iso_path.is_none());
    }

    #[test]
    fn test_backwards_compatible_deserialization() {
        // V1.0 profile JSON without conversion state fields
        let v1_json = r#"{
            "version": "1.0",
            "profile_name": "Old Profile",
            "created": "2024-01-01T00:00:00Z",
            "modified": "2024-01-01T00:00:00Z",
            "folders": ["/music/album"],
            "settings": {
                "target_bitrate": "auto",
                "no_lossy_conversions": false,
                "embed_album_art": true
            },
            "volume_label": null
        }"#;

        let profile: BurnProfile = serde_json::from_str(v1_json).unwrap();

        assert_eq!(profile.profile_name, "Old Profile");
        assert!(!profile.has_conversion_state());
        // New fields should default to None
        assert!(profile.session_id.is_none());
        assert!(profile.folder_states.is_none());
    }

    #[test]
    fn test_saved_folder_state_source_modified() {
        let state = SavedFolderState::new(
            "abc123".to_string(),
            "abc123".to_string(),
            Some(320),
            50_000_000,
            1234567890,
            10,
        );

        // Same mtime - not modified
        assert!(!state.source_modified(1234567890));

        // Different mtime - modified
        assert!(state.source_modified(1234567891));
    }
}
