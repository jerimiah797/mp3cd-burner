//! Application state types
//!
//! Contains shared state types used across the application:
//! - AppSettings: Global application preferences
//! - BurnSettings: Settings for a burn operation
//! - BurnStage: Current stage of the burn process
//! - ConversionState: Thread-safe state for conversion progress
//! - ImportState: Thread-safe state for folder import progress

#![allow(dead_code)]

use gpui::Global;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::MusicFolder;

/// Application-wide settings
///
/// Persisted to ~/Library/Application Support/MP3 CD Burner/app_settings.json
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct AppSettings {
    /// Whether to simulate burning (don't actually burn)
    #[serde(default)]
    pub simulate_burn: bool,
    /// Whether to avoid lossy-to-lossy conversions
    #[serde(default)]
    pub no_lossy_conversions: bool,
    /// Whether to embed album art in MP3s
    #[serde(default)]
    pub embed_album_art: bool,
}


impl Global for AppSettings {}

impl AppSettings {
    const SETTINGS_FILE: &'static str = "app_settings.json";

    /// Get the app data directory (~/Library/Application Support/MP3 CD Burner/)
    fn get_app_data_dir() -> Result<PathBuf, String> {
        let data_dir =
            dirs::data_dir().ok_or_else(|| "Could not determine data directory".to_string())?;

        let app_dir = data_dir.join("MP3 CD Burner");

        // Create directory if it doesn't exist
        if !app_dir.exists() {
            std::fs::create_dir_all(&app_dir)
                .map_err(|e| format!("Failed to create app data directory: {}", e))?;
        }

        Ok(app_dir)
    }

    /// Load app settings from disk, or return defaults if not found
    pub fn load() -> Self {
        match Self::try_load() {
            Ok(settings) => {
                log::debug!("Loaded app settings from disk");
                settings
            }
            Err(e) => {
                log::debug!("Using default app settings: {}", e);
                Self::default()
            }
        }
    }

    fn try_load() -> Result<Self, String> {
        let app_dir = Self::get_app_data_dir()?;
        let settings_path = app_dir.join(Self::SETTINGS_FILE);

        if !settings_path.exists() {
            return Err("Settings file not found".to_string());
        }

        let contents = std::fs::read_to_string(&settings_path)
            .map_err(|e| format!("Failed to read settings: {}", e))?;

        serde_json::from_str(&contents).map_err(|e| format!("Failed to parse settings: {}", e))
    }

    /// Save app settings to disk
    pub fn save(&self) -> Result<(), String> {
        let app_dir = Self::get_app_data_dir()?;
        let settings_path = app_dir.join(Self::SETTINGS_FILE);

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;

        std::fs::write(&settings_path, json)
            .map_err(|e| format!("Failed to write settings: {}", e))?;

        log::debug!("Saved app settings to {:?}", settings_path);
        Ok(())
    }
}

/// Window state for position/size persistence
///
/// Persisted to ~/Library/Application Support/MP3 CD Burner/window_state.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    /// Window X position
    pub x: f64,
    /// Window Y position
    pub y: f64,
    /// Window width
    pub width: f64,
    /// Window height
    pub height: f64,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: 100.0,
            y: 100.0,
            width: 600.0,
            height: 500.0,
        }
    }
}

impl WindowState {
    const STATE_FILE: &'static str = "window_state.json";

    /// Get the app data directory (~/Library/Application Support/MP3 CD Burner/)
    fn get_app_data_dir() -> Result<PathBuf, String> {
        let data_dir =
            dirs::data_dir().ok_or_else(|| "Could not determine data directory".to_string())?;

        let app_dir = data_dir.join("MP3 CD Burner");

        // Create directory if it doesn't exist
        if !app_dir.exists() {
            std::fs::create_dir_all(&app_dir)
                .map_err(|e| format!("Failed to create app data directory: {}", e))?;
        }

        Ok(app_dir)
    }

    /// Load window state from disk, or return defaults if not found
    pub fn load() -> Self {
        match Self::try_load() {
            Ok(state) => {
                log::debug!(
                    "Loaded window state from disk: {}x{} at ({}, {})",
                    state.width, state.height, state.x, state.y
                );
                state
            }
            Err(e) => {
                log::debug!("Using default window state: {}", e);
                Self::default()
            }
        }
    }

    fn try_load() -> Result<Self, String> {
        let app_dir = Self::get_app_data_dir()?;
        let state_path = app_dir.join(Self::STATE_FILE);

        if !state_path.exists() {
            return Err("State file not found".to_string());
        }

        let contents = std::fs::read_to_string(&state_path)
            .map_err(|e| format!("Failed to read state: {}", e))?;

        serde_json::from_str(&contents).map_err(|e| format!("Failed to parse state: {}", e))
    }

    /// Save window state to disk
    pub fn save(&self) -> Result<(), String> {
        let app_dir = Self::get_app_data_dir()?;
        let state_path = app_dir.join(Self::STATE_FILE);

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize state: {}", e))?;

        std::fs::write(&state_path, json).map_err(|e| format!("Failed to write state: {}", e))?;

        Ok(())
    }
}

/// Display settings for folder list items
///
/// Controls which details are shown for each folder in the list.
/// Persisted to ~/Library/Application Support/MP3 CD Burner/display_settings.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplaySettings {
    /// Show file count (e.g., "12 files")
    pub show_file_count: bool,
    /// Show original size (e.g., "500 MB")
    pub show_original_size: bool,
    /// Show converted size (e.g., "→ 180 MB")
    pub show_converted_size: bool,
    /// Show source format (e.g., "FLAC" or "MP3/AAC")
    pub show_source_format: bool,
    /// Show source bitrate (e.g., "320k" or "128-320k")
    pub show_source_bitrate: bool,
    /// Show final bitrate after conversion (e.g., "@192k")
    pub show_final_bitrate: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            // Default to verbose in debug builds, sparse in release
            show_file_count: cfg!(debug_assertions),
            show_original_size: cfg!(debug_assertions),
            show_converted_size: cfg!(debug_assertions),
            show_source_format: cfg!(debug_assertions),
            show_source_bitrate: cfg!(debug_assertions),
            show_final_bitrate: cfg!(debug_assertions),
        }
    }
}

impl Global for DisplaySettings {}

impl DisplaySettings {
    const SETTINGS_FILE: &'static str = "display_settings.json";

    /// Get the app data directory (~/Library/Application Support/MP3 CD Burner/)
    fn get_app_data_dir() -> Result<PathBuf, String> {
        let data_dir =
            dirs::data_dir().ok_or_else(|| "Could not determine data directory".to_string())?;

        let app_dir = data_dir.join("MP3 CD Burner");

        // Create directory if it doesn't exist
        if !app_dir.exists() {
            std::fs::create_dir_all(&app_dir)
                .map_err(|e| format!("Failed to create app data directory: {}", e))?;
        }

        Ok(app_dir)
    }

    /// Load display settings from disk, or return defaults if not found
    pub fn load() -> Self {
        match Self::try_load() {
            Ok(settings) => {
                log::debug!("Loaded display settings from disk");
                settings
            }
            Err(e) => {
                log::debug!("Using default display settings: {}", e);
                Self::default()
            }
        }
    }

    fn try_load() -> Result<Self, String> {
        let app_dir = Self::get_app_data_dir()?;
        let settings_path = app_dir.join(Self::SETTINGS_FILE);

        if !settings_path.exists() {
            return Err("Settings file not found".to_string());
        }

        let contents = std::fs::read_to_string(&settings_path)
            .map_err(|e| format!("Failed to read settings: {}", e))?;

        serde_json::from_str(&contents).map_err(|e| format!("Failed to parse settings: {}", e))
    }

    /// Save display settings to disk
    pub fn save(&self) -> Result<(), String> {
        let app_dir = Self::get_app_data_dir()?;
        let settings_path = app_dir.join(Self::SETTINGS_FILE);

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;

        std::fs::write(&settings_path, json)
            .map_err(|e| format!("Failed to write settings: {}", e))?;

        log::debug!("Saved display settings to {:?}", settings_path);
        Ok(())
    }
}

/// Settings for a burn operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurnSettings {
    /// Target bitrate in kbps
    pub bitrate: u32,
    /// Volume label for the CD
    pub volume_label: String,
}

impl Default for BurnSettings {
    fn default() -> Self {
        Self {
            bitrate: 192,
            volume_label: String::new(),
        }
    }
}

/// Current stage of the burn process
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurnStage {
    /// Converting audio files
    Converting,
    /// Creating ISO image
    CreatingIso,
    /// Waiting for user to insert a blank CD
    WaitingForCd,
    /// Detected an erasable disc (CD-RW) with data - waiting for user to confirm erase
    ErasableDiscDetected,
    /// Erasing CD-RW before burning
    Erasing,
    /// Burning ISO to CD
    Burning,
    /// Finishing up (closing session, verifying)
    Finishing,
    /// Process complete (success or simulated)
    Complete,
    /// Process was cancelled
    Cancelled,
}

impl BurnStage {
    pub fn display_text(&self) -> &'static str {
        match self {
            BurnStage::Converting => "Converting...",
            BurnStage::CreatingIso => "Creating ISO...",
            BurnStage::WaitingForCd => "Insert blank CD",
            BurnStage::ErasableDiscDetected => "Erase disc?",
            BurnStage::Erasing => "Erasing...",
            BurnStage::Burning => "Burning...",
            BurnStage::Finishing => "Finishing...",
            BurnStage::Complete => "Complete!",
            BurnStage::Cancelled => "Cancelled",
        }
    }
}

/// Shared state for tracking conversion progress across threads
#[derive(Clone)]
pub struct ConversionState {
    /// Whether conversion is currently running
    pub is_converting: Arc<AtomicBool>,
    /// Whether cancellation has been requested
    pub cancel_requested: Arc<AtomicBool>,
    /// Whether user has approved erasing a CD-RW
    pub erase_approved: Arc<AtomicBool>,
    /// Number of files completed
    pub completed: Arc<AtomicUsize>,
    /// Number of files failed
    pub failed: Arc<AtomicUsize>,
    /// Total number of files to convert
    pub total: Arc<AtomicUsize>,
    /// Current stage of the burn process
    pub stage: Arc<Mutex<BurnStage>>,
    /// Burn progress percentage (0-100, or -1 for indeterminate)
    pub burn_progress: Arc<AtomicI32>,
    /// Path to the created ISO (for re-burning)
    pub iso_path: Arc<Mutex<Option<PathBuf>>>,
}

impl Global for ConversionState {}

impl ConversionState {
    pub fn new() -> Self {
        Self {
            is_converting: Arc::new(AtomicBool::new(false)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            erase_approved: Arc::new(AtomicBool::new(false)),
            completed: Arc::new(AtomicUsize::new(0)),
            failed: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(0)),
            stage: Arc::new(Mutex::new(BurnStage::Converting)),
            burn_progress: Arc::new(AtomicI32::new(-1)),
            iso_path: Arc::new(Mutex::new(None)),
        }
    }

    pub fn reset(&self, total: usize) {
        self.is_converting.store(true, Ordering::SeqCst);
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.erase_approved.store(false, Ordering::SeqCst);
        self.completed.store(0, Ordering::SeqCst);
        self.failed.store(0, Ordering::SeqCst);
        self.total.store(total, Ordering::SeqCst);
        *self.stage.lock().unwrap() = BurnStage::Converting;
        self.burn_progress.store(-1, Ordering::SeqCst);
        *self.iso_path.lock().unwrap() = None;
    }

    pub fn finish(&self) {
        self.is_converting.store(false, Ordering::SeqCst);
    }

    pub fn set_stage(&self, stage: BurnStage) {
        *self.stage.lock().unwrap() = stage;
    }

    pub fn get_stage(&self) -> BurnStage {
        *self.stage.lock().unwrap()
    }

    pub fn set_burn_progress(&self, progress: i32) {
        self.burn_progress.store(progress, Ordering::SeqCst);
    }

    pub fn get_burn_progress(&self) -> i32 {
        self.burn_progress.load(Ordering::SeqCst)
    }

    /// Request cancellation of the current conversion
    pub fn request_cancel(&self) {
        self.cancel_requested.store(true, Ordering::SeqCst);
    }

    /// Check if cancellation has been requested
    pub fn is_cancelled(&self) -> bool {
        self.cancel_requested.load(Ordering::SeqCst)
    }

    pub fn is_converting(&self) -> bool {
        self.is_converting.load(Ordering::SeqCst)
    }

    pub fn progress(&self) -> (usize, usize, usize) {
        (
            self.completed.load(Ordering::SeqCst),
            self.failed.load(Ordering::SeqCst),
            self.total.load(Ordering::SeqCst),
        )
    }
}

impl Default for ConversionState {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared state for tracking folder import progress across threads
#[derive(Clone)]
pub struct ImportState {
    /// Whether import is currently running
    pub is_importing: Arc<AtomicBool>,
    /// Number of folders scanned
    pub completed: Arc<AtomicUsize>,
    /// Total number of folders to scan
    pub total: Arc<AtomicUsize>,
    /// Scanned folders waiting to be added to the list
    pub scanned_folders: Arc<Mutex<Vec<MusicFolder>>>,
    /// Folder paths that failed to load (for error reporting)
    pub failed_paths: Arc<Mutex<Vec<PathBuf>>>,
}

impl ImportState {
    pub fn new() -> Self {
        Self {
            is_importing: Arc::new(AtomicBool::new(false)),
            completed: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(0)),
            scanned_folders: Arc::new(Mutex::new(Vec::new())),
            failed_paths: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn reset(&self, total: usize) {
        self.is_importing.store(true, Ordering::SeqCst);
        self.completed.store(0, Ordering::SeqCst);
        self.total.store(total, Ordering::SeqCst);
        self.scanned_folders.lock().unwrap().clear();
        self.failed_paths.lock().unwrap().clear();
    }

    pub fn finish(&self) {
        self.is_importing.store(false, Ordering::SeqCst);
    }

    pub fn is_importing(&self) -> bool {
        self.is_importing.load(Ordering::SeqCst)
    }

    pub fn progress(&self) -> (usize, usize) {
        (
            self.completed.load(Ordering::SeqCst),
            self.total.load(Ordering::SeqCst),
        )
    }

    /// Push a scanned folder to the queue
    pub fn push_folder(&self, folder: MusicFolder) {
        self.scanned_folders.lock().unwrap().push(folder);
        self.completed.fetch_add(1, Ordering::SeqCst);
    }

    /// Record a failed folder path
    pub fn push_failed(&self, path: PathBuf) {
        self.failed_paths.lock().unwrap().push(path);
        self.completed.fetch_add(1, Ordering::SeqCst);
    }

    /// Get all failed paths
    pub fn get_failed_paths(&self) -> Vec<PathBuf> {
        self.failed_paths.lock().unwrap().clone()
    }

    /// Drain all scanned folders from the queue
    pub fn drain_folders(&self) -> Vec<MusicFolder> {
        let mut folders = self.scanned_folders.lock().unwrap();
        std::mem::take(&mut *folders)
    }

    /// Check if there are folders waiting to be drained
    pub fn has_pending_folders(&self) -> bool {
        !self.scanned_folders.lock().unwrap().is_empty()
    }
}

impl Default for ImportState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_settings_default() {
        let settings = AppSettings::default();
        assert!(!settings.simulate_burn);
        assert!(!settings.no_lossy_conversions);
        assert!(!settings.embed_album_art);
    }

    #[test]
    fn test_app_settings_serialize() {
        let settings = AppSettings {
            simulate_burn: true,
            no_lossy_conversions: true,
            embed_album_art: true,
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("simulate_burn"));
        assert!(json.contains("true"));
    }

    #[test]
    fn test_app_settings_deserialize() {
        let json = r#"{"simulate_burn":true,"no_lossy_conversions":false,"embed_album_art":true}"#;
        let settings: AppSettings = serde_json::from_str(json).unwrap();
        assert!(settings.simulate_burn);
        assert!(!settings.no_lossy_conversions);
        assert!(settings.embed_album_art);
    }

    #[test]
    fn test_burn_settings_default() {
        let settings = BurnSettings::default();
        assert_eq!(settings.bitrate, 192);
        assert!(settings.volume_label.is_empty());
    }

    #[test]
    fn test_burn_settings_custom() {
        let settings = BurnSettings {
            bitrate: 320,
            volume_label: "My Music".to_string(),
        };
        assert_eq!(settings.bitrate, 320);
        assert_eq!(settings.volume_label, "My Music");
    }

    #[test]
    fn test_window_state_default() {
        let state = WindowState::default();
        assert_eq!(state.x, 100.0);
        assert_eq!(state.y, 100.0);
        assert_eq!(state.width, 600.0);
        assert_eq!(state.height, 500.0);
    }

    #[test]
    fn test_window_state_serialize() {
        let state = WindowState {
            x: 200.0,
            y: 150.0,
            width: 800.0,
            height: 600.0,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: WindowState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.x, 200.0);
        assert_eq!(parsed.width, 800.0);
    }

    #[test]
    fn test_display_settings_default() {
        let settings = DisplaySettings::default();
        // All fields should be same value (based on debug_assertions)
        assert_eq!(
            settings.show_file_count,
            settings.show_original_size
        );
        assert_eq!(
            settings.show_converted_size,
            settings.show_source_format
        );
    }

    #[test]
    fn test_display_settings_serialize() {
        let settings = DisplaySettings {
            show_file_count: true,
            show_original_size: false,
            show_converted_size: true,
            show_source_format: false,
            show_source_bitrate: true,
            show_final_bitrate: false,
        };
        let json = serde_json::to_string(&settings).unwrap();
        let parsed: DisplaySettings = serde_json::from_str(&json).unwrap();
        assert!(parsed.show_file_count);
        assert!(!parsed.show_original_size);
        assert!(parsed.show_converted_size);
        assert!(!parsed.show_source_format);
    }

    #[test]
    fn test_burn_stage_display_text() {
        assert_eq!(BurnStage::Converting.display_text(), "Converting...");
        assert_eq!(BurnStage::CreatingIso.display_text(), "Creating ISO...");
        assert_eq!(BurnStage::WaitingForCd.display_text(), "Insert blank CD");
        assert_eq!(BurnStage::ErasableDiscDetected.display_text(), "Erase disc?");
        assert_eq!(BurnStage::Erasing.display_text(), "Erasing...");
        assert_eq!(BurnStage::Burning.display_text(), "Burning...");
        assert_eq!(BurnStage::Finishing.display_text(), "Finishing...");
        assert_eq!(BurnStage::Complete.display_text(), "Complete!");
        assert_eq!(BurnStage::Cancelled.display_text(), "Cancelled");
    }

    #[test]
    fn test_conversion_state_new() {
        let state = ConversionState::new();
        assert!(!state.is_converting());
        assert!(!state.is_cancelled());
        let (completed, failed, total) = state.progress();
        assert_eq!(completed, 0);
        assert_eq!(failed, 0);
        assert_eq!(total, 0);
    }

    #[test]
    fn test_conversion_state_default() {
        let state = ConversionState::default();
        assert!(!state.is_converting());
    }

    #[test]
    fn test_conversion_state_reset() {
        let state = ConversionState::new();
        state.reset(10);
        assert!(state.is_converting());
        assert!(!state.is_cancelled());
        let (completed, failed, total) = state.progress();
        assert_eq!(completed, 0);
        assert_eq!(failed, 0);
        assert_eq!(total, 10);
        assert_eq!(state.get_stage(), BurnStage::Converting);
        assert_eq!(state.get_burn_progress(), -1);
    }

    #[test]
    fn test_conversion_state_finish() {
        let state = ConversionState::new();
        state.reset(10);
        assert!(state.is_converting());
        state.finish();
        assert!(!state.is_converting());
    }

    #[test]
    fn test_conversion_state_cancel() {
        let state = ConversionState::new();
        assert!(!state.is_cancelled());
        state.request_cancel();
        assert!(state.is_cancelled());
    }

    #[test]
    fn test_conversion_state_stage() {
        let state = ConversionState::new();
        state.set_stage(BurnStage::Burning);
        assert_eq!(state.get_stage(), BurnStage::Burning);
        state.set_stage(BurnStage::Complete);
        assert_eq!(state.get_stage(), BurnStage::Complete);
    }

    #[test]
    fn test_conversion_state_burn_progress() {
        let state = ConversionState::new();
        state.set_burn_progress(50);
        assert_eq!(state.get_burn_progress(), 50);
        state.set_burn_progress(100);
        assert_eq!(state.get_burn_progress(), 100);
    }

    #[test]
    fn test_conversion_state_progress_tracking() {
        let state = ConversionState::new();
        state.reset(5);
        state.completed.fetch_add(2, Ordering::SeqCst);
        state.failed.fetch_add(1, Ordering::SeqCst);
        let (completed, failed, total) = state.progress();
        assert_eq!(completed, 2);
        assert_eq!(failed, 1);
        assert_eq!(total, 5);
    }

    #[test]
    fn test_conversion_state_clone() {
        let state1 = ConversionState::new();
        state1.reset(10);
        state1.set_stage(BurnStage::Burning);

        let state2 = state1.clone();
        assert!(state2.is_converting());
        assert_eq!(state2.get_stage(), BurnStage::Burning);

        // Changes to state1 should be visible in state2 (Arc)
        state1.set_stage(BurnStage::Complete);
        assert_eq!(state2.get_stage(), BurnStage::Complete);
    }

    #[test]
    fn test_import_state_new() {
        let state = ImportState::new();
        assert!(!state.is_importing());
        let (completed, total) = state.progress();
        assert_eq!(completed, 0);
        assert_eq!(total, 0);
    }

    #[test]
    fn test_import_state_default() {
        let state = ImportState::default();
        assert!(!state.is_importing());
    }

    #[test]
    fn test_import_state_reset() {
        let state = ImportState::new();
        state.reset(5);
        assert!(state.is_importing());
        let (completed, total) = state.progress();
        assert_eq!(completed, 0);
        assert_eq!(total, 5);
    }

    #[test]
    fn test_import_state_finish() {
        let state = ImportState::new();
        state.reset(5);
        assert!(state.is_importing());
        state.finish();
        assert!(!state.is_importing());
    }

    #[test]
    fn test_import_state_push_and_drain_folders() {
        let state = ImportState::new();
        state.reset(2);

        // Push some folders
        let folder1 = MusicFolder::new_for_test("/test/album1");
        let folder2 = MusicFolder::new_for_test("/test/album2");
        state.push_folder(folder1);
        state.push_folder(folder2);

        assert!(state.has_pending_folders());
        let (completed, _) = state.progress();
        assert_eq!(completed, 2);

        // Drain
        let folders = state.drain_folders();
        assert_eq!(folders.len(), 2);
        assert!(!state.has_pending_folders());
    }

    #[test]
    fn test_import_state_push_failed() {
        let state = ImportState::new();
        state.reset(2);

        state.push_failed(PathBuf::from("/bad/path1"));
        state.push_failed(PathBuf::from("/bad/path2"));

        let failed = state.get_failed_paths();
        assert_eq!(failed.len(), 2);
        assert_eq!(failed[0], PathBuf::from("/bad/path1"));
        let (completed, _) = state.progress();
        assert_eq!(completed, 2);
    }

    #[test]
    fn test_import_state_clone() {
        let state1 = ImportState::new();
        state1.reset(5);

        let state2 = state1.clone();
        assert!(state2.is_importing());

        // Changes to state1 should be visible in state2 (Arc)
        state1.finish();
        assert!(!state2.is_importing());
    }

    #[test]
    fn test_burn_stage_equality() {
        assert_eq!(BurnStage::Converting, BurnStage::Converting);
        assert_ne!(BurnStage::Converting, BurnStage::Burning);
    }

    #[test]
    fn test_burn_stage_copy() {
        let stage = BurnStage::Complete;
        let stage_copy = stage;
        assert_eq!(stage, stage_copy);
    }

    #[test]
    fn test_conversion_state_erase_approved() {
        let state = ConversionState::new();
        assert!(!state.erase_approved.load(Ordering::SeqCst));
        state.erase_approved.store(true, Ordering::SeqCst);
        assert!(state.erase_approved.load(Ordering::SeqCst));
    }

    #[test]
    fn test_conversion_state_iso_path() {
        let state = ConversionState::new();
        assert!(state.iso_path.lock().unwrap().is_none());

        *state.iso_path.lock().unwrap() = Some(PathBuf::from("/tmp/test.iso"));
        assert_eq!(
            state.iso_path.lock().unwrap().as_ref().unwrap().to_str().unwrap(),
            "/tmp/test.iso"
        );
    }

    #[test]
    fn test_conversion_state_reset_clears_iso_path() {
        let state = ConversionState::new();
        *state.iso_path.lock().unwrap() = Some(PathBuf::from("/tmp/old.iso"));
        state.reset(5);
        assert!(state.iso_path.lock().unwrap().is_none());
    }

    #[test]
    fn test_conversion_state_reset_clears_erase_approved() {
        let state = ConversionState::new();
        state.erase_approved.store(true, Ordering::SeqCst);
        state.reset(5);
        assert!(!state.erase_approved.load(Ordering::SeqCst));
    }

    #[test]
    fn test_import_state_has_pending_folders_empty() {
        let state = ImportState::new();
        assert!(!state.has_pending_folders());
    }

    #[test]
    fn test_import_state_drain_empty() {
        let state = ImportState::new();
        let folders = state.drain_folders();
        assert!(folders.is_empty());
    }

    #[test]
    fn test_import_state_reset_clears_previous() {
        let state = ImportState::new();
        state.reset(3);
        state.push_folder(MusicFolder::new_for_test("/test/album"));
        state.push_failed(PathBuf::from("/bad/path"));

        // Reset should clear everything
        state.reset(5);
        assert!(!state.has_pending_folders());
        assert!(state.get_failed_paths().is_empty());
        assert_eq!(state.progress().0, 0);
        assert_eq!(state.progress().1, 5);
    }

    #[test]
    fn test_burn_stage_debug() {
        let stage = BurnStage::ErasableDiscDetected;
        let debug_str = format!("{:?}", stage);
        assert!(debug_str.contains("ErasableDiscDetected"));
    }

    #[test]
    fn test_burn_settings_clone() {
        let settings = BurnSettings {
            bitrate: 256,
            volume_label: "Test Label".to_string(),
        };
        let cloned = settings.clone();
        assert_eq!(cloned.bitrate, 256);
        assert_eq!(cloned.volume_label, "Test Label");
    }

    #[test]
    fn test_app_settings_clone() {
        let settings = AppSettings {
            simulate_burn: true,
            no_lossy_conversions: true,
            embed_album_art: false,
        };
        let cloned = settings.clone();
        assert!(cloned.simulate_burn);
        assert!(cloned.no_lossy_conversions);
        assert!(!cloned.embed_album_art);
    }

    #[test]
    fn test_window_state_clone() {
        let state = WindowState {
            x: 150.0,
            y: 200.0,
            width: 1000.0,
            height: 800.0,
        };
        let cloned = state.clone();
        assert_eq!(cloned.x, 150.0);
        assert_eq!(cloned.y, 200.0);
        assert_eq!(cloned.width, 1000.0);
        assert_eq!(cloned.height, 800.0);
    }

    #[test]
    fn test_display_settings_clone() {
        let settings = DisplaySettings {
            show_file_count: true,
            show_original_size: false,
            show_converted_size: true,
            show_source_format: false,
            show_source_bitrate: true,
            show_final_bitrate: false,
        };
        let cloned = settings.clone();
        assert!(cloned.show_file_count);
        assert!(!cloned.show_original_size);
    }
}
