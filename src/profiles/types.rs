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

/// Saved audio track for mixtape serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedMixtapeTrack {
    /// Source file path
    pub source_path: String,
    /// Duration in seconds
    pub duration: f64,
    /// Bitrate in kbps
    pub bitrate: u32,
    /// File size in bytes
    pub size: u64,
    /// Audio codec name
    pub codec: String,
    /// Whether this is a lossy format
    pub is_lossy: bool,
    /// Album art as base64-encoded image data (per-track for mixtapes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_art_base64: Option<String>,
}

/// Kind of folder in a saved profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedFolderKind {
    /// Regular album folder scanned from filesystem
    Album {
        /// Paths of tracks excluded from burn
        #[serde(default)]
        excluded_tracks: Vec<String>,
        /// Custom track order (indices into original audio_files)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track_order: Option<Vec<usize>>,
    },
    /// User-created mixtape/playlist
    Mixtape {
        /// Mixtape name
        name: String,
        /// Ordered list of tracks
        tracks: Vec<SavedMixtapeTrack>,
    },
}

impl Default for SavedFolderKind {
    fn default() -> Self {
        SavedFolderKind::Album {
            excluded_tracks: Vec::new(),
            track_order: None,
        }
    }
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

    // === Display metadata (v2.1+) - allows loading without source ===
    /// Album name from audio file metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_name: Option<String>,

    /// Artist name from audio file metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_name: Option<String>,

    /// Release year from audio file metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<String>,

    /// Total duration of all audio files in seconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_duration: Option<f64>,

    /// Album art as base64-encoded image data
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_art: Option<String>,

    /// Total size of source files in bytes (for display)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_size: Option<u64>,

    /// When this folder was converted (Unix timestamp)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<u64>,

    /// Folder kind (Album with exclusions/order, or Mixtape with tracks)
    #[serde(default)]
    pub kind: SavedFolderKind,
}

impl SavedFolderState {
    /// Create a new SavedFolderState with basic fields (for backwards compat)
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
            // Display metadata defaults
            album_name: None,
            artist_name: None,
            year: None,
            total_duration: None,
            album_art: None,
            source_size: None,
            completed_at: None,
            kind: SavedFolderKind::default(),
        }
    }

    /// Create a SavedFolderState with full display metadata
    #[allow(clippy::too_many_arguments)]
    pub fn with_metadata(
        folder_id: String,
        output_dir: String,
        lossless_bitrate: Option<u32>,
        output_size: u64,
        source_mtime: u64,
        file_count: usize,
        album_name: Option<String>,
        artist_name: Option<String>,
        year: Option<String>,
        total_duration: Option<f64>,
        album_art: Option<String>,
        source_size: Option<u64>,
        completed_at: Option<u64>,
        kind: Option<SavedFolderKind>,
    ) -> Self {
        Self {
            folder_id,
            output_dir,
            lossless_bitrate,
            output_size,
            source_mtime,
            file_count,
            album_name,
            artist_name,
            year,
            total_duration,
            album_art,
            source_size,
            completed_at,
            kind: kind.unwrap_or_default(),
        }
    }

    /// Check if the source folder has been modified since encoding
    pub fn source_modified(&self, current_mtime: u64) -> bool {
        current_mtime != self.source_mtime
    }

    /// Check if this state has display metadata (v2.1+ format)
    pub fn has_display_metadata(&self) -> bool {
        self.total_duration.is_some()
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

    #[test]
    fn test_saved_folder_state_with_metadata() {
        let state = SavedFolderState::with_metadata(
            "folder123".to_string(),
            "output/folder123".to_string(),
            Some(256),
            75_000_000,
            1700000000,
            15,
            Some("OK Computer".to_string()),
            Some("Radiohead".to_string()),
            Some("1997".to_string()),
            Some(3200.5),
            Some("base64albumart".to_string()),
            Some(100_000_000),
            Some(1700000100),
            None,
        );

        assert_eq!(state.folder_id, "folder123");
        assert_eq!(state.album_name, Some("OK Computer".to_string()));
        assert_eq!(state.artist_name, Some("Radiohead".to_string()));
        assert_eq!(state.year, Some("1997".to_string()));
        assert_eq!(state.total_duration, Some(3200.5));
        assert!(state.has_display_metadata());
    }

    #[test]
    fn test_saved_folder_state_has_display_metadata() {
        let state_without = SavedFolderState::new(
            "abc".to_string(),
            "out".to_string(),
            None,
            1000,
            100,
            5,
        );
        assert!(!state_without.has_display_metadata());

        let state_with = SavedFolderState::with_metadata(
            "abc".to_string(),
            "out".to_string(),
            None,
            1000,
            100,
            5,
            None,
            None,
            None,
            Some(120.0), // has total_duration
            None,
            None,
            None,
            None,
        );
        assert!(state_with.has_display_metadata());
    }

    #[test]
    fn test_saved_folder_kind_default() {
        let kind = SavedFolderKind::default();
        match kind {
            SavedFolderKind::Album {
                excluded_tracks,
                track_order,
            } => {
                assert!(excluded_tracks.is_empty());
                assert!(track_order.is_none());
            }
            _ => panic!("Default should be Album"),
        }
    }

    #[test]
    fn test_saved_folder_kind_album_with_exclusions() {
        let kind = SavedFolderKind::Album {
            excluded_tracks: vec!["/path/to/track1.mp3".to_string()],
            track_order: Some(vec![2, 0, 1]),
        };

        match kind {
            SavedFolderKind::Album {
                excluded_tracks,
                track_order,
            } => {
                assert_eq!(excluded_tracks.len(), 1);
                assert_eq!(track_order, Some(vec![2, 0, 1]));
            }
            _ => panic!("Should be Album"),
        }
    }

    #[test]
    fn test_saved_folder_kind_mixtape() {
        let track = SavedMixtapeTrack {
            source_path: "/music/song.mp3".to_string(),
            duration: 240.5,
            bitrate: 320,
            size: 9_600_000,
            codec: "mp3".to_string(),
            is_lossy: true,
            album_art_base64: None,
        };

        let kind = SavedFolderKind::Mixtape {
            name: "Road Trip Mix".to_string(),
            tracks: vec![track],
        };

        match kind {
            SavedFolderKind::Mixtape { name, tracks } => {
                assert_eq!(name, "Road Trip Mix");
                assert_eq!(tracks.len(), 1);
                assert_eq!(tracks[0].duration, 240.5);
            }
            _ => panic!("Should be Mixtape"),
        }
    }

    #[test]
    fn test_saved_mixtape_track_serialization() {
        let track = SavedMixtapeTrack {
            source_path: "/music/song.flac".to_string(),
            duration: 300.0,
            bitrate: 1411,
            size: 52_912_500,
            codec: "flac".to_string(),
            is_lossy: false,
            album_art_base64: Some("abc123base64".to_string()),
        };

        let json = serde_json::to_string(&track).unwrap();
        let deserialized: SavedMixtapeTrack = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.source_path, "/music/song.flac");
        assert_eq!(deserialized.duration, 300.0);
        assert!(!deserialized.is_lossy);
        assert_eq!(deserialized.album_art_base64, Some("abc123base64".to_string()));
    }

    #[test]
    fn test_burn_settings_clone() {
        let settings = BurnSettings {
            target_bitrate: "256".to_string(),
            no_lossy_conversions: true,
            embed_album_art: false,
        };

        let cloned = settings.clone();
        assert_eq!(cloned.target_bitrate, "256");
        assert!(cloned.no_lossy_conversions);
        assert!(!cloned.embed_album_art);
    }

    #[test]
    fn test_burn_profile_touch() {
        let settings = BurnSettings {
            target_bitrate: "auto".to_string(),
            no_lossy_conversions: false,
            embed_album_art: true,
        };

        let mut profile = BurnProfile::new("Test".to_string(), vec![], settings);
        let original_modified = profile.modified.clone();

        // Small delay to ensure timestamp changes
        std::thread::sleep(std::time::Duration::from_millis(10));

        profile.touch();

        // Modified timestamp should have changed
        assert_ne!(profile.modified, original_modified);
    }

    #[test]
    fn test_conversion_state_validation_fields() {
        let validation = ConversionStateValidation {
            session_exists: true,
            valid_folders: vec!["/folder1".to_string(), "/folder2".to_string()],
            invalid_folders: vec!["/folder3".to_string()],
            iso_valid: false,
        };

        assert!(validation.session_exists);
        assert_eq!(validation.valid_folders.len(), 2);
        assert_eq!(validation.invalid_folders.len(), 1);
        assert!(!validation.iso_valid);
    }

    #[test]
    fn test_burn_profile_with_volume_label() {
        let settings = BurnSettings {
            target_bitrate: "auto".to_string(),
            no_lossy_conversions: false,
            embed_album_art: true,
        };

        let mut profile = BurnProfile::new("Test".to_string(), vec![], settings);
        profile.volume_label = Some("MY_CD_LABEL".to_string());

        assert_eq!(profile.volume_label, Some("MY_CD_LABEL".to_string()));
    }

    #[test]
    fn test_burn_profile_with_bitrate_override() {
        let settings = BurnSettings {
            target_bitrate: "auto".to_string(),
            no_lossy_conversions: false,
            embed_album_art: true,
        };

        let mut profile = BurnProfile::new("Test".to_string(), vec![], settings);
        profile.manual_bitrate_override = Some(192);

        assert_eq!(profile.manual_bitrate_override, Some(192));
    }

    #[test]
    fn test_saved_folder_kind_serialization() {
        let kind = SavedFolderKind::Album {
            excluded_tracks: vec!["track1.mp3".to_string()],
            track_order: Some(vec![1, 0, 2]),
        };

        let json = serde_json::to_string(&kind).unwrap();
        let deserialized: SavedFolderKind = serde_json::from_str(&json).unwrap();

        match deserialized {
            SavedFolderKind::Album {
                excluded_tracks,
                track_order,
            } => {
                assert_eq!(excluded_tracks, vec!["track1.mp3"]);
                assert_eq!(track_order, Some(vec![1, 0, 2]));
            }
            _ => panic!("Should deserialize to Album"),
        }
    }

    #[test]
    fn test_saved_folder_state_clone() {
        let state = SavedFolderState::new(
            "id1".to_string(),
            "out1".to_string(),
            Some(320),
            1_000_000,
            123456,
            10,
        );

        let cloned = state.clone();
        assert_eq!(cloned.folder_id, "id1");
        assert_eq!(cloned.output_dir, "out1");
        assert_eq!(cloned.lossless_bitrate, Some(320));
    }
}
