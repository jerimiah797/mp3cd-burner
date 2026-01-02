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
#[derive(Debug, Clone, Default)]
pub struct AppSettings {
    /// Whether to simulate burning (don't actually burn)
    pub simulate_burn: bool,
    /// Whether to avoid lossy-to-lossy conversions
    pub no_lossy_conversions: bool,
    /// Whether to embed album art in MP3s
    pub embed_album_art: bool,
}

impl Global for AppSettings {}

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
        let data_dir = dirs::data_dir()
            .ok_or_else(|| "Could not determine data directory".to_string())?;

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
                println!("Loaded display settings from disk");
                settings
            }
            Err(e) => {
                println!("Using default display settings: {}", e);
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

        serde_json::from_str(&contents)
            .map_err(|e| format!("Failed to parse settings: {}", e))
    }

    /// Save display settings to disk
    pub fn save(&self) -> Result<(), String> {
        let app_dir = Self::get_app_data_dir()?;
        let settings_path = app_dir.join(Self::SETTINGS_FILE);

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;

        std::fs::write(&settings_path, json)
            .map_err(|e| format!("Failed to write settings: {}", e))?;

        println!("Saved display settings to {:?}", settings_path);
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
}

impl ImportState {
    pub fn new() -> Self {
        Self {
            is_importing: Arc::new(AtomicBool::new(false)),
            completed: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(0)),
            scanned_folders: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn reset(&self, total: usize) {
        self.is_importing.store(true, Ordering::SeqCst);
        self.completed.store(0, Ordering::SeqCst);
        self.total.store(total, Ordering::SeqCst);
        self.scanned_folders.lock().unwrap().clear();
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

    /// Drain all scanned folders from the queue
    pub fn drain_folders(&self) -> Vec<MusicFolder> {
        let mut folders = self.scanned_folders.lock().unwrap();
        std::mem::take(&mut *folders)
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
    fn test_burn_settings_default() {
        let settings = BurnSettings::default();
        assert_eq!(settings.bitrate, 192);
        assert!(settings.volume_label.is_empty());
    }
}
