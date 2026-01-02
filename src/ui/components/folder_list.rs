//! FolderList component - The main application view with folder list
//!
//! This is currently the root view of the application, containing:
//! - Header
//! - Folder list with drag-and-drop
//! - Status bar

use gpui::{div, prelude::*, rgb, AnyWindowHandle, AsyncApp, Context, ExternalPaths, FocusHandle, IntoElement, PathPromptOptions, PromptLevel, Render, ScrollHandle, SharedString, Timer, WeakEntity, Window};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::actions::{NewProfile, OpenProfile, SaveProfile};

use super::folder_item::{render_folder_item, DraggedFolder, FolderItemProps};
use super::status_bar::{
    get_stage_color, is_stage_cancelable, render_import_progress, render_iso_too_large,
    render_stats_panel, ProgressDisplay, StatusBarState,
};
use crate::burning::{
    coordinate_burn, create_iso, BurnConfig,
    IsoAction, IsoState, determine_iso_action,
};
use crate::conversion::{calculate_multipass_bitrate, BackgroundEncoderHandle, OutputManager};
use crate::core::{
    find_album_folders, scan_music_folder,
    AppSettings, BurnStage, ConversionState, FolderConversionStatus, FolderId, ImportState, MusicFolder,
};
use crate::ui::Theme;

/// The main folder list view
///
/// Handles:
/// - Displaying the list of folders
/// - External drag-drop from Finder (ExternalPaths)
/// - Internal drag-drop for reordering
/// - Empty state rendering
pub struct FolderList {
    /// The list of scanned music folders
    folders: Vec<MusicFolder>,
    /// Currently hovered drop target index (for visual feedback)
    drop_target_index: Option<usize>,
    /// Whether we've subscribed to appearance changes
    appearance_subscription_set: bool,
    /// Handle for scroll state
    scroll_handle: ScrollHandle,
    /// Conversion progress state
    conversion_state: ConversionState,
    /// Import progress state
    import_state: ImportState,
    /// Focus handle for receiving actions (None in tests)
    focus_handle: Option<FocusHandle>,
    /// Background encoder handle for immediate conversion (None until initialized)
    background_encoder: Option<BackgroundEncoderHandle>,
    /// Event receiver for background encoder progress updates (std::sync::mpsc for easy polling)
    encoder_event_rx: Option<std::sync::mpsc::Receiver<crate::conversion::EncoderEvent>>,
    /// Output manager for session-based directories (None until initialized)
    #[allow(dead_code)]
    output_manager: Option<OutputManager>,
    /// Current ISO state (for "Burn Another" functionality)
    iso_state: Option<IsoState>,
    /// Whether auto-ISO generation has been attempted (prevents retry loop on failure)
    iso_generation_attempted: bool,
    /// Whether the current ISO has been burned at least once (for "Burn Another" vs "Burn")
    iso_has_been_burned: bool,
    /// Timestamp of last folder list change (for debounced bitrate recalculation)
    last_folder_change: Option<std::time::Instant>,
    /// Last calculated bitrate (to detect changes that require re-encoding)
    last_calculated_bitrate: Option<u32>,
    /// Whether we need to grab initial focus (for menu items to work)
    needs_initial_focus: bool,
}

impl FolderList {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
            appearance_subscription_set: false,
            scroll_handle: ScrollHandle::new(),
            conversion_state: ConversionState::new(),
            import_state: ImportState::new(),
            focus_handle: Some(cx.focus_handle()),
            background_encoder: None,
            encoder_event_rx: None,
            output_manager: None,
            iso_state: None,
            iso_generation_attempted: false,
            iso_has_been_burned: false,
            last_folder_change: None,
            last_calculated_bitrate: None,
            needs_initial_focus: true,
        }
    }

    /// Create a new FolderList for testing (without GPUI context)
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
            appearance_subscription_set: false,
            scroll_handle: ScrollHandle::new(),
            conversion_state: ConversionState::new(),
            import_state: ImportState::new(),
            focus_handle: None,
            background_encoder: None,
            encoder_event_rx: None,
            output_manager: None,
            iso_state: None,
            iso_generation_attempted: false,
            iso_has_been_burned: false,
            last_folder_change: None,
            last_calculated_bitrate: None,
            needs_initial_focus: false,
        }
    }

    /// Initialize the background encoder for immediate folder conversion
    ///
    /// This should be called after construction when background encoding is desired.
    /// If not called, folders will only be converted when "Burn" is clicked (legacy mode).
    pub fn enable_background_encoding(&mut self) -> Result<(), String> {
        use crate::conversion::BackgroundEncoder;

        // Create the background encoder (this spawns its own thread with Tokio runtime)
        // IMPORTANT: Use the output_manager returned by the encoder - it's the same one
        // used for encoding, so ISO staging will find the encoded files!
        let (_encoder, handle, event_rx, output_manager) = BackgroundEncoder::new()?;

        // Clean up old sessions from previous runs
        output_manager.cleanup_old_sessions()?;

        // Store the handle, event receiver, and output manager
        self.background_encoder = Some(handle);
        self.encoder_event_rx = Some(event_rx);
        self.output_manager = Some(output_manager);

        println!("Background encoding enabled, session: {:?}",
            self.output_manager.as_ref().map(|m| m.session_id()));

        Ok(())
    }

    /// Start polling for encoder events (called after enabling background encoding)
    ///
    /// This must be called with a GPUI context to start the polling loop.
    pub fn start_encoder_polling(&self, cx: &mut Context<Self>) {
        // Start the polling loop
        Self::start_encoder_event_polling(cx);
    }

    /// Set the background encoder handle (for use from async context)
    #[allow(dead_code)]
    pub fn set_background_encoder(&mut self, handle: BackgroundEncoderHandle) {
        self.background_encoder = Some(handle);
    }

    /// Check if background encoding is available
    #[allow(dead_code)]
    pub fn has_background_encoder(&self) -> bool {
        self.background_encoder.is_some()
    }

    /// Get the output manager if available
    #[allow(dead_code)]
    pub fn output_manager(&self) -> Option<&OutputManager> {
        self.output_manager.as_ref()
    }

    /// Queue a folder for background encoding (if encoder is available)
    #[allow(dead_code)]
    fn queue_folder_for_encoding(&self, folder: &MusicFolder) {
        if let Some(ref encoder) = self.background_encoder {
            encoder.add_folder(folder.clone());
        }
    }

    /// Notify encoder that a folder was removed
    #[allow(dead_code)]
    fn notify_folder_removed(&self, folder: &MusicFolder) {
        if let Some(ref encoder) = self.background_encoder {
            encoder.remove_folder(&folder.id);
        }
    }

    /// Notify encoder that folders were reordered
    #[allow(dead_code)]
    fn notify_folders_reordered(&self) {
        if let Some(ref encoder) = self.background_encoder {
            encoder.folders_reordered();
        }
    }

    /// Get the conversion status of a specific folder
    #[allow(dead_code)]
    pub fn get_folder_conversion_status(&self, folder_id: &crate::core::FolderId) -> FolderConversionStatus {
        if let Some(ref encoder) = self.background_encoder {
            let state = encoder.get_state();
            let guard = state.lock().unwrap();

            // Check if completed
            if let Some((status, _folder)) = guard.completed.get(folder_id) {
                return status.clone();
            }

            // Check if active
            if let Some((active_id, _, _)) = &guard.active {
                if active_id == folder_id {
                    return FolderConversionStatus::Converting {
                        files_completed: 0,
                        files_total: 0,
                    };
                }
            }

            // Check if queued
            if guard.queue.iter().any(|(id, _)| id == folder_id) {
                return FolderConversionStatus::NotConverted;
            }
        }

        FolderConversionStatus::NotConverted
    }

    /// Get the current ISO state
    #[allow(dead_code)]
    pub fn iso_state(&self) -> Option<&IsoState> {
        self.iso_state.as_ref()
    }

    /// Set the ISO state after successful burn
    #[allow(dead_code)]
    pub fn set_iso_state(&mut self, iso_state: IsoState) {
        self.iso_state = Some(iso_state);
    }

    /// Clear the ISO state (e.g., when starting fresh)
    #[allow(dead_code)]
    pub fn clear_iso_state(&mut self) {
        self.iso_state = None;
    }

    /// Check if "Burn Another" is available
    ///
    /// Returns true if we have a valid ISO that matches current folders.
    pub fn can_burn_another(&self) -> bool {
        match &self.iso_state {
            Some(iso) => iso.is_ready_to_burn(&self.folders),
            None => false,
        }
    }

    /// Check if the current ISO exceeds the CD size limit
    ///
    /// Returns true if we have an ISO but it's too large for a CD.
    pub fn iso_exceeds_limit(&self) -> bool {
        match &self.iso_state {
            Some(iso) => iso.exceeds_cd_limit(),
            None => false,
        }
    }

    /// Get the ISO size in MB (for display)
    pub fn iso_size_mb(&self) -> Option<f64> {
        self.iso_state.as_ref().map(|iso| iso.size_bytes as f64 / (1024.0 * 1024.0))
    }

    /// Get the list of encoded folder IDs (from background encoder state)
    fn get_encoded_folder_ids(&self) -> Vec<FolderId> {
        if let Some(ref encoder) = self.background_encoder {
            let state = encoder.get_state();
            let guard = state.lock().unwrap();
            guard.completed.keys().cloned().collect()
        } else {
            // In legacy mode (no background encoder), we don't track this
            vec![]
        }
    }

    /// Determine what action is needed for the current burn request
    #[allow(dead_code)]
    pub fn determine_burn_action(&self) -> IsoAction {
        let encoded_ids = self.get_encoded_folder_ids();
        determine_iso_action(self.iso_state.as_ref(), &self.folders, &encoded_ids)
    }

    /// Check if all folders are ready (converted) for burning
    pub fn all_folders_converted(&self) -> bool {
        if self.folders.is_empty() {
            return false;
        }

        if let Some(ref encoder) = self.background_encoder {
            let state = encoder.get_state();
            let guard = state.lock().unwrap();

            self.folders.iter().all(|folder| {
                guard.completed.contains_key(&folder.id)
            })
        } else {
            // In legacy mode, assume not converted until burn process runs
            false
        }
    }

    /// Poll encoder events and handle them
    ///
    /// Returns true if any events were processed (useful for knowing if UI needs refresh)
    fn poll_encoder_events(&mut self) -> bool {
        use crate::conversion::EncoderEvent;

        let rx = match self.encoder_event_rx.as_ref() {
            Some(rx) => rx,
            None => return false,
        };

        let mut events_processed = false;

        // Drain all available events
        while let Ok(event) = rx.try_recv() {
            events_processed = true;

            match event {
                EncoderEvent::FolderStarted { id, files_total } => {
                    println!("Encoding started: {:?} ({} files)", id, files_total);
                }
                EncoderEvent::FolderProgress { id, files_completed, files_total } => {
                    // Could update per-folder progress UI here
                    println!("Encoding progress: {:?} {}/{}", id, files_completed, files_total);
                }
                EncoderEvent::FolderCompleted { id, output_dir, output_size, lossless_bitrate } => {
                    println!("Encoding complete: {:?} -> {:?} ({} bytes, bitrate: {:?})",
                        id, output_dir, output_size, lossless_bitrate);

                    // Update the folder's conversion status
                    if let Some(folder) = self.folders.iter_mut().find(|f| f.id == id) {
                        folder.conversion_status = FolderConversionStatus::Converted {
                            output_dir,
                            lossless_bitrate,
                            output_size,
                            completed_at: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs(),
                        };
                    }
                }
                EncoderEvent::FolderFailed { id, error } => {
                    eprintln!("Encoding failed: {:?} - {}", id, error);
                }
                EncoderEvent::FolderCancelled(id) => {
                    println!("Encoding cancelled: {:?}", id);
                }
                EncoderEvent::BitrateRecalculated { new_bitrate, reencode_needed } => {
                    println!("Bitrate recalculated to {} kbps, {} folders need re-encoding",
                        new_bitrate, reencode_needed.len());
                    // Invalidate ISO state - output files are being regenerated
                    self.iso_state = None;
                    self.iso_generation_attempted = false;
                }
            }
        }

        events_processed
    }

    /// Check if ISO needs to be generated and do it if ready
    ///
    /// This should be called periodically to auto-generate ISO when all folders are encoded.
    /// Returns true if ISO generation was triggered.
    fn maybe_generate_iso(&mut self, cx: &mut Context<Self>) -> bool {
        // Don't generate if we already have a valid ISO
        if self.can_burn_another() {
            return false;
        }

        // Don't generate if we already attempted (prevents infinite retry on failure)
        // This flag is reset when folders change
        if self.iso_generation_attempted {
            return false;
        }

        // Don't generate if no folders or not all converted
        if self.folders.is_empty() || !self.all_folders_converted() {
            return false;
        }

        // Don't generate if already converting/burning
        if self.conversion_state.is_converting() {
            return false;
        }

        // Mark as attempted to prevent retry loop
        self.iso_generation_attempted = true;

        println!("All folders encoded - generating ISO automatically...");

        // Get the output manager to access encoded folder paths
        let output_manager = match &self.output_manager {
            Some(om) => om.clone(),
            None => return false,
        };

        // Build staging directory with numbered symlinks
        let folders_for_staging: Vec<_> = self.folders.iter().cloned().collect();

        // Spawn background thread to create ISO
        let folders = folders_for_staging.clone();
        let state = self.conversion_state.clone();

        // Mark as converting (for ISO stage)
        state.reset(0);
        state.set_stage(BurnStage::CreatingIso);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async {
                // Create staging directory with symlinks
                match output_manager.create_iso_staging(&folders) {
                    Ok(staging_dir) => {
                        println!("ISO staging directory: {:?}", staging_dir);

                        // Create ISO from staging directory
                        let volume_label = "MP3CD".to_string();
                        match crate::burning::create_iso(&staging_dir, &volume_label) {
                            Ok(result) => {
                                println!("ISO created successfully: {:?}", result.iso_path);
                                *state.iso_path.lock().unwrap() = Some(result.iso_path);
                                state.set_stage(BurnStage::Complete);
                            }
                            Err(e) => {
                                eprintln!("ISO creation failed: {}", e);
                                state.set_stage(BurnStage::Complete);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to create ISO staging: {}", e);
                        state.set_stage(BurnStage::Complete);
                    }
                }
                state.finish();
            });
        });

        // Start polling for ISO creation progress
        Self::start_iso_creation_polling(self.conversion_state.clone(), folders_for_staging, cx);

        cx.notify();
        true
    }

    /// Start polling for ISO creation completion (lightweight - just waits for it to finish)
    fn start_iso_creation_polling(
        state: ConversionState,
        folders: Vec<MusicFolder>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                loop {
                    let cx_for_after_await = async_cx.clone();
                    Timer::after(std::time::Duration::from_millis(100)).await;

                    // Check if ISO creation is done
                    if !state.is_converting() {
                        break;
                    }

                    let _ = cx_for_after_await.refresh();
                    async_cx = cx_for_after_await;
                }

                // ISO creation finished - save iso_state
                let iso_path = state.iso_path.lock().unwrap().clone();
                if let Some(path) = iso_path {
                    let _ = this.update(&mut async_cx, |folder_list, _cx| {
                        if let Ok(iso_state) = IsoState::new(path.clone(), &folders) {
                            folder_list.iso_state = Some(iso_state);
                            println!("ISO state saved - ready for Burn");
                        }
                    });
                }

                let _ = async_cx.refresh();
            }
        })
        .detach();
    }

    /// Returns the number of folders in the list
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.folders.len()
    }

    /// Returns true if the list is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }

    /// Returns an iterator over the folders
    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = &MusicFolder> {
        self.folders.iter()
    }

    /// Check if folder path is already in the list
    fn contains_path(&self, path: &PathBuf) -> bool {
        self.folders.iter().any(|f| f.path == *path)
    }

    /// Add folders from external drop (Finder)
    ///
    /// Scans each folder asynchronously in a background thread.
    /// Only adds directories that aren't already in the list.
    pub fn add_external_folders(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        // Don't start if already importing
        if self.import_state.is_importing() {
            println!("Import already in progress");
            return;
        }

        // Filter to only new directories (check on main thread before spawning)
        let new_paths: Vec<PathBuf> = paths
            .iter()
            .filter(|p| p.is_dir() && !self.contains_path(p))
            .cloned()
            .collect();

        if new_paths.is_empty() {
            return;
        }

        println!("Starting async import of {} folders", new_paths.len());

        // Reset import state (total will be updated after expansion)
        self.import_state.reset(new_paths.len());

        // Clone state for background thread
        let state = self.import_state.clone();

        // Spawn background thread for scanning
        std::thread::spawn(move || {
            // Expand each path into album folders (smart detection)
            let album_paths: Vec<PathBuf> = new_paths
                .iter()
                .flat_map(|p| find_album_folders(p))
                .collect();

            println!("Expanded to {} album folders", album_paths.len());

            // Reset state with actual count
            state.total.store(album_paths.len(), Ordering::SeqCst);

            for path in album_paths {
                println!("Scanning: {}", path.display());
                match scan_music_folder(&path) {
                    Ok(folder) => {
                        println!(
                            "Scanned folder: {} ({} files, {} bytes)",
                            folder.path.display(),
                            folder.file_count,
                            folder.total_size
                        );
                        state.push_folder(folder);
                    }
                    Err(e) => {
                        eprintln!("Failed to scan folder {}: {}", path.display(), e);
                        // Still increment completed so we know when done
                        state.completed.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            state.finish();
            println!("Import complete");
        });

        // Start polling for results
        Self::start_import_polling(self.import_state.clone(), cx);
    }

    /// Add a single folder to the list
    #[allow(dead_code)]
    pub fn add_folder(&mut self, path: PathBuf) {
        if path.is_dir() && !self.contains_path(&path) {
            if let Ok(folder) = scan_music_folder(&path) {
                // Queue for background encoding if available
                self.queue_folder_for_encoding(&folder);
                self.folders.push(folder);
                // Invalidate ISO since folder list changed
                self.iso_state = None;
                self.iso_generation_attempted = false;
                self.iso_has_been_burned = false;
                // Record change time for debounced bitrate recalculation
                self.last_folder_change = Some(std::time::Instant::now());
            }
        }
    }

    /// Remove a folder by index
    pub fn remove_folder(&mut self, index: usize) {
        if index < self.folders.len() {
            let folder = self.folders.remove(index);
            // Notify encoder if available
            self.notify_folder_removed(&folder);
            // Invalidate ISO since folder list changed
            self.iso_state = None;
            self.iso_generation_attempted = false;
            self.iso_has_been_burned = false;
            // Record change time for debounced bitrate recalculation
            self.last_folder_change = Some(std::time::Instant::now());
        }
    }

    /// Move a folder from one index to another
    pub fn move_folder(&mut self, from: usize, to: usize) {
        if from < self.folders.len() && to <= self.folders.len() && from != to {
            let folder = self.folders.remove(from);
            let insert_at = if to > from { to - 1 } else { to };
            self.folders.insert(insert_at, folder);
            // Notify encoder about reorder
            self.notify_folders_reordered();
            // Invalidate ISO since folder order changed (will need regeneration)
            self.iso_state = None;
            self.iso_generation_attempted = false;
            self.iso_has_been_burned = false;
        }
    }

    /// Clear all folders
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.folders.clear();
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.iso_has_been_burned = false;
    }

    /// Get all folder paths (for saving profiles, etc.)
    #[allow(dead_code)]
    pub fn get_folder_paths(&self) -> Vec<PathBuf> {
        self.folders.iter().map(|f| f.path.clone()).collect()
    }

    /// Get all folders
    #[allow(dead_code)]
    pub fn get_folders(&self) -> &[MusicFolder] {
        &self.folders
    }

    /// Set folders from a saved profile (re-scans each folder)
    #[allow(dead_code)]
    pub fn set_folders(&mut self, paths: Vec<PathBuf>) {
        self.folders.clear();
        self.iso_state = None;
        for path in paths {
            if let Ok(folder) = scan_music_folder(&path) {
                self.folders.push(folder);
            }
        }
    }

    /// Create a BurnProfile from the current state
    ///
    /// This captures the current folder list and conversion state,
    /// allowing the profile to be saved and later restored.
    pub fn create_profile(&self, profile_name: String) -> crate::profiles::BurnProfile {
        use crate::profiles::{BurnProfile, BurnSettings, SavedFolderState};
        use std::collections::HashMap;

        let settings = BurnSettings {
            target_bitrate: "auto".to_string(),
            no_lossy_conversions: false,
            embed_album_art: true,
        };

        let folder_paths: Vec<String> = self.folders
            .iter()
            .map(|f| f.path.to_string_lossy().to_string())
            .collect();

        let mut profile = BurnProfile::new(profile_name, folder_paths, settings);

        // Add conversion state if we have it
        if let Some(ref output_manager) = self.output_manager {
            let session_id = output_manager.session_id().to_string();

            // Build folder states map
            let mut folder_states = HashMap::new();
            for folder in &self.folders {
                if let FolderConversionStatus::Converted { output_dir, lossless_bitrate, output_size, .. } = &folder.conversion_status {
                    // Get source folder mtime
                    let source_mtime = std::fs::metadata(&folder.path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    let saved_state = SavedFolderState::new(
                        folder.id.0.clone(),
                        output_dir.to_string_lossy().to_string(),
                        *lossless_bitrate,
                        *output_size,
                        source_mtime,
                        folder.file_count as usize,
                    );
                    folder_states.insert(folder.path.to_string_lossy().to_string(), saved_state);
                }
            }

            // Get ISO info if available
            let (iso_path, iso_hash) = match &self.iso_state {
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

        profile
    }

    /// Save the current state as a profile to the specified path
    pub fn save_profile(&self, path: &std::path::Path, profile_name: String) -> Result<(), String> {
        let profile = self.create_profile(profile_name);
        crate::profiles::storage::save_profile(&profile, path)?;
        crate::profiles::storage::add_to_recent_profiles(&path.to_string_lossy())?;
        println!("Profile saved to: {}", path.display());
        Ok(())
    }

    /// Load a profile and restore its state
    ///
    /// This will:
    /// 1. Load the profile from disk
    /// 2. Validate the saved conversion state
    /// 3. Restore folders with valid conversion state
    /// 4. Queue folders needing re-encoding to the background encoder
    pub fn load_profile(&mut self, path: &std::path::Path, cx: &mut Context<Self>) -> Result<(), String> {
        use crate::profiles::storage::{load_profile, validate_conversion_state, add_to_recent_profiles};

        let profile = load_profile(path)?;
        let validation = validate_conversion_state(&profile);

        println!("Loading profile: {}", profile.profile_name);
        println!("  Valid folders: {:?}", validation.valid_folders);
        println!("  Invalid folders: {:?}", validation.invalid_folders);
        println!("  ISO valid: {}", validation.iso_valid);

        // Clear current state
        self.folders.clear();
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.iso_has_been_burned = false;

        // Track folders that need encoding (don't queue yet - need to calculate bitrate first)
        let mut folders_needing_encoding: Vec<MusicFolder> = Vec::new();

        // Load folders
        for folder_path_str in &profile.folders {
            let folder_path = PathBuf::from(folder_path_str);
            if let Ok(mut folder) = scan_music_folder(&folder_path) {
                // Check if this folder has valid saved state
                if validation.valid_folders.contains(folder_path_str) {
                    // Restore conversion status from saved state
                    if let Some(ref folder_states) = profile.folder_states {
                        if let Some(saved) = folder_states.get(folder_path_str) {
                            folder.conversion_status = FolderConversionStatus::Converted {
                                output_dir: PathBuf::from(&saved.output_dir),
                                lossless_bitrate: saved.lossless_bitrate,
                                output_size: saved.output_size,
                                completed_at: 0, // Not stored in v1.1
                            };
                        }
                    }
                } else {
                    // Needs encoding - track it for later
                    folders_needing_encoding.push(folder.clone());
                }

                self.folders.push(folder);
            }
        }

        // Now that all folders are loaded, calculate the correct bitrate BEFORE queueing
        if !folders_needing_encoding.is_empty() {
            let target_bitrate = self.calculated_bitrate();
            println!("Profile loaded - calculated bitrate: {} kbps", target_bitrate);

            // Update encoder with correct bitrate before queueing folders
            if let Some(ref encoder) = self.background_encoder {
                encoder.recalculate_bitrate(target_bitrate);
            }

            // Now queue all folders that need encoding (with correct bitrate)
            for folder in folders_needing_encoding {
                self.queue_folder_for_encoding(&folder);
            }

            // Set last_folder_change so debounced recalc doesn't override with stale value
            self.last_folder_change = Some(std::time::Instant::now());
            self.last_calculated_bitrate = Some(target_bitrate);
        }

        // Restore ISO state if valid
        if validation.iso_valid {
            if let Some(ref iso_path_str) = profile.iso_path {
                if let Ok(iso_state) = IsoState::new(PathBuf::from(iso_path_str), &self.folders) {
                    self.iso_state = Some(iso_state);
                    println!("Restored ISO state from profile");
                }
            }
        }

        // Update recent profiles
        let _ = add_to_recent_profiles(&path.to_string_lossy());

        cx.notify();
        Ok(())
    }

    /// Clear current state for a new profile (called from File > New menu)
    pub fn new_profile(&mut self, cx: &mut Context<Self>) {
        self.folders.clear();
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.iso_has_been_burned = false;
        println!("New profile - cleared all folders");
        cx.notify();
    }

    /// Show file picker to open a profile (called from File > Open menu)
    pub fn open_profile(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        };
        let receiver = cx.prompt_for_paths(options);
        cx.spawn(|this_handle: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                if let Ok(Ok(Some(paths))) = receiver.await {
                    if let Some(path) = paths.first() {
                        let path = path.clone();
                        let _ = this_handle.update(&mut async_cx, |this, cx| {
                            if let Err(e) = this.load_profile(&path, cx) {
                                eprintln!("Failed to load profile: {}", e);
                            }
                        });
                    }
                }
            }
        }).detach();
    }

    /// Show save dialog to save current profile (called from File > Save menu)
    pub fn save_profile_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.folders.is_empty() {
            println!("No folders to save");
            return;
        }

        // Generate a default filename from the first folder
        let default_name = self.folders.first()
            .and_then(|f| f.path.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());

        let documents_dir = dirs::document_dir()
            .unwrap_or_else(|| PathBuf::from("."));

        let receiver = cx.prompt_for_new_path(&documents_dir, Some(&format!("{}.burn", default_name)));
        let profile_name = default_name.clone();
        cx.spawn(|this_handle: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                if let Ok(Ok(Some(path))) = receiver.await {
                    let _ = this_handle.update(&mut async_cx, |this, _cx| {
                        if let Err(e) = this.save_profile(&path, profile_name) {
                            eprintln!("Failed to save profile: {}", e);
                        } else {
                            println!("Profile saved to: {:?}", path);
                        }
                    });
                }
            }
        }).detach();
    }

    /// Calculate total files across all folders
    pub fn total_files(&self) -> u32 {
        self.folders.iter().map(|f| f.file_count).sum()
    }

    /// Calculate total size across all folders
    pub fn total_size(&self) -> u64 {
        self.folders.iter().map(|f| f.total_size).sum()
    }

    /// Calculate total duration across all folders (in seconds)
    pub fn total_duration(&self) -> f64 {
        self.folders.iter().map(|f| f.total_duration).sum()
    }

    /// Calculate the optimal bitrate to fit on a 700MB CD
    ///
    /// Uses multi-pass-aware calculation:
    /// - MP3s are copied (exact size)
    /// - Lossy files transcoded at source bitrate
    /// - Lossless files get remaining space
    ///
    /// Returns the full estimate with bitrate and display logic
    pub fn calculated_bitrate_estimate(&self) -> Option<crate::conversion::MultipassEstimate> {
        if self.folders.is_empty() {
            return None;
        }

        // Collect all audio files from cached folder data
        let all_files: Vec<_> = self.folders
            .iter()
            .flat_map(|f| f.audio_files.iter().cloned())
            .collect();

        if all_files.is_empty() {
            return None;
        }

        // Use multi-pass-aware calculation
        Some(calculate_multipass_bitrate(&all_files))
    }

    /// Get the target bitrate for display (convenience wrapper)
    pub fn calculated_bitrate(&self) -> u32 {
        self.calculated_bitrate_estimate()
            .map(|e| e.target_bitrate)
            .unwrap_or(320)
    }

    /// Check if debounce period has passed and trigger bitrate recalculation
    ///
    /// This is called from the encoder polling loop. When folder list changes:
    /// 1. Wait 500ms (debounce) to let rapid additions settle
    /// 2. Calculate new target bitrate
    /// 3. If bitrate changed, send recalculate command to encoder
    fn check_debounced_bitrate_recalculation(&mut self) {
        const DEBOUNCE_MS: u64 = 500;

        // Check if we have a pending change that's old enough
        let should_recalculate = match self.last_folder_change {
            Some(change_time) => {
                change_time.elapsed() >= std::time::Duration::from_millis(DEBOUNCE_MS)
            }
            None => false,
        };

        if !should_recalculate {
            return;
        }

        // Clear the pending change
        self.last_folder_change = None;

        // Skip if no folders
        if self.folders.is_empty() {
            self.last_calculated_bitrate = None;
            return;
        }

        // Calculate new bitrate
        let new_bitrate = self.calculated_bitrate();

        // Check if bitrate changed
        let bitrate_changed = match self.last_calculated_bitrate {
            Some(old) => old != new_bitrate,
            None => true, // First calculation
        };

        // Update stored bitrate
        self.last_calculated_bitrate = Some(new_bitrate);

        if !bitrate_changed {
            return;
        }

        println!("Bitrate recalculated: {:?} -> {} kbps",
            self.last_calculated_bitrate.map(|b| format!("{}", b)).unwrap_or_else(|| "None".to_string()),
            new_bitrate);

        // Send recalculation command to background encoder
        if let Some(ref encoder) = self.background_encoder {
            encoder.recalculate_bitrate(new_bitrate);
        }
    }

    /// Render the empty state drop zone
    fn render_empty_state(&self, theme: &Theme) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(theme.text_muted)
            .child(div().text_2xl().child("📂"))
            .child(div().text_lg().child("Drop music folders here"))
            .child(div().text_sm().child("or drag items to reorder"))
    }

    /// Render the populated folder list
    fn render_folder_items(&mut self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let drop_target = self.drop_target_index;
        let mut list = div().w_full().flex().flex_col().gap_2();

        for (index, folder) in self.folders.iter().enumerate() {
            let props = FolderItemProps {
                index,
                folder: folder.clone(),
                is_drop_target: drop_target == Some(index),
                theme: *theme,
            };

            let item = render_folder_item(
                props,
                cx,
                |view: &mut Self, from, to| {
                    view.move_folder(from, to);
                    view.drop_target_index = None;
                },
                |view: &mut Self, idx| {
                    view.remove_folder(idx);
                },
            );

            list = list.child(item);
        }

        list
    }
}

impl Render for FolderList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Subscribe to appearance changes and register action handlers (once)
        if !self.appearance_subscription_set {
            self.appearance_subscription_set = true;
            cx.observe_window_appearance(window, |_this, _window, cx| {
                cx.notify();
            })
            .detach();
        }

        // Grab initial focus so menu items work immediately
        if self.needs_initial_focus {
            self.needs_initial_focus = false;
            if let Some(ref focus_handle) = self.focus_handle {
                focus_handle.focus(window);
            }
        }

        // Get theme based on OS appearance
        let theme = Theme::from_appearance(window.appearance());
        let is_empty = self.folders.is_empty();

        // Build the folder list content
        let list_content = if is_empty {
            self.render_empty_state(&theme).into_any_element()
        } else {
            self.render_folder_items(&theme, cx).into_any_element()
        };

        // Capture all listeners first (before borrowing for status bar)
        let on_external_drop = cx.listener(|this, paths: &ExternalPaths, _window, cx| {
            this.add_external_folders(paths.paths(), cx);
            this.drop_target_index = None;
        });

        let on_internal_drop = cx.listener(|this, dragged: &DraggedFolder, _window, _cx| {
            let target = this.folders.len();
            this.move_folder(dragged.index, target);
            this.drop_target_index = None;
        });

        // Profile action handlers
        let on_new_profile = cx.listener(|this, _: &NewProfile, _window, cx| {
            this.new_profile(cx);
        });
        let on_open_profile = cx.listener(|this, _: &OpenProfile, window, cx| {
            this.open_profile(window, cx);
        });
        let on_save_profile = cx.listener(|this, _: &SaveProfile, window, cx| {
            this.save_profile_dialog(window, cx);
        });

        // Build status bar after listeners
        let status_bar = self.render_status_bar(&theme, cx);

        // Build the base container
        let mut container = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg);

        // Track focus if we have a focus handle (not in tests)
        if let Some(ref focus_handle) = self.focus_handle {
            container = container.track_focus(focus_handle);
        }

        container
            .on_action(on_new_profile)
            .on_action(on_open_profile)
            .on_action(on_save_profile)
            // Handle external file drops on the entire window
            .on_drop(on_external_drop)
            // Style when dragging external files over window
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(0x3d3d3d)))
            // Main content area - folder list (scrollable)
            .child(
                div()
                    .id("folder-list-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll_handle)
                    .px_6() // Horizontal padding for breathing room
                    .py_2() // Vertical padding
                    // Handle drops on the list container
                    .on_drop(on_internal_drop)
                    .drag_over::<DraggedFolder>(|style, _, _, _| style.bg(rgb(0x3d3d3d)))
                    .child(list_content),
            )
            // Status bar at bottom
            .child(status_bar)
    }
}

impl FolderList {
    /// Build the StatusBarState from current FolderList state
    fn build_status_bar_state(&self) -> StatusBarState {
        StatusBarState {
            total_files: self.total_files(),
            total_size: self.total_size(),
            total_duration: self.total_duration(),
            bitrate_estimate: self.calculated_bitrate_estimate(),
            has_folders: !self.folders.is_empty(),
            is_importing: self.import_state.is_importing(),
            import_progress: self.import_state.progress(),
            is_converting: self.conversion_state.is_converting(),
            conversion_progress: self.conversion_state.progress(),
            burn_stage: self.conversion_state.get_stage(),
            burn_progress: self.conversion_state.get_burn_progress(),
            is_cancelled: self.conversion_state.is_cancelled(),
            can_burn_another: self.can_burn_another(),
            iso_exceeds_limit: self.iso_exceeds_limit(),
            iso_size_mb: self.iso_size_mb(),
            iso_has_been_burned: self.iso_has_been_burned,
        }
    }

    /// Render the status bar with detailed stats and action button
    fn render_status_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.build_status_bar_state();
        let success_color = theme.success;
        let success_hover = theme.success_hover;
        let text_muted = theme.text_muted;

        div()
            .py_4()
            .px_6()
            .flex()
            .items_center()
            .justify_between()
            .bg(theme.bg)
            .border_t_1()
            .border_color(theme.border)
            .text_sm()
            // Left side: stats panel (delegated to helper)
            .child(render_stats_panel(&state, theme))
            // Right side: action panel
            .child(self.render_action_panel(&state, theme, success_color, success_hover, text_muted, cx))
    }

    /// Render the right action panel (progress displays and buttons)
    fn render_action_panel(
        &self,
        state: &StatusBarState,
        theme: &Theme,
        success_color: gpui::Hsla,
        success_hover: gpui::Hsla,
        text_muted: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if state.is_importing {
            render_import_progress(state, theme).into_any_element()
        } else if state.is_converting {
            self.render_conversion_progress(state, theme, success_color, success_hover, cx)
                .into_any_element()
        } else if state.can_burn_another && state.iso_exceeds_limit {
            render_iso_too_large(state.iso_size_mb.unwrap_or(0.0), theme).into_any_element()
        } else if state.can_burn_another {
            self.render_burn_button(state.iso_has_been_burned, success_color, success_hover, cx)
                .into_any_element()
        } else {
            self.render_convert_burn_button(state.has_folders, success_color, success_hover, text_muted, cx)
                .into_any_element()
        }
    }

    /// Render conversion/burn progress with cancel support
    fn render_conversion_progress(
        &self,
        state: &StatusBarState,
        theme: &Theme,
        success_color: gpui::Hsla,
        success_hover: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let progress = ProgressDisplay::from_state(state);
        let stage_color = get_stage_color(state, theme);
        let is_cancelable = is_stage_cancelable(state);

        div()
            .id(SharedString::from("convert-progress-container"))
            .flex()
            .flex_col()
            .gap_2()
            .items_center()
            // Progress display (hide when waiting for user to approve erase)
            .when(state.burn_stage != BurnStage::ErasableDiscDetected, |el| {
                el.child(
                    div()
                        .id(SharedString::from("progress-display"))
                        .w(gpui::px(150.0))
                        .h(gpui::px(70.0))
                        .rounded_md()
                        .border_1()
                        .border_color(stage_color)
                        .overflow_hidden()
                        .relative()
                        .when(is_cancelable, |el| {
                            el.cursor_pointer()
                                .on_click(cx.listener(|this, _event, _window, _cx| {
                                    this.conversion_state.request_cancel();
                                }))
                        })
                        // Background progress fill
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(gpui::relative(progress.fraction))
                                .bg(stage_color),
                        )
                        // Text overlay
                        .child(
                            div()
                                .size_full()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .relative()
                                .when(!progress.text.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .text_lg()
                                            .text_color(gpui::white())
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(progress.text.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .text_lg()
                                        .text_color(gpui::white())
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(
                                            if state.is_cancelled && state.burn_stage != BurnStage::Cancelled {
                                                "Cancelling..."
                                            } else {
                                                progress.stage_text
                                            },
                                        ),
                                ),
                        ),
                )
            })
            // Erase & Burn button (only show when erasable disc detected)
            .when(state.burn_stage == BurnStage::ErasableDiscDetected, |el| {
                el.child(
                    div()
                        .id(SharedString::from("erase-burn-btn"))
                        .w(gpui::px(150.0))
                        .h(gpui::px(70.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(success_color)
                        .text_color(gpui::white())
                        .text_lg()
                        .rounded_md()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(success_hover))
                        .on_click(cx.listener(|this, _event, _window, _cx| {
                            println!("Erase & Burn clicked");
                            this.conversion_state.erase_approved.store(true, Ordering::SeqCst);
                        }))
                        .child("Erase\n& Burn"),
                )
            })
    }

    /// Render Burn/Burn Another button
    fn render_burn_button(
        &self,
        iso_has_been_burned: bool,
        success_color: gpui::Hsla,
        success_hover: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let button_text = if iso_has_been_burned { "Burn\nAnother" } else { "Burn" };
        let button_id = if iso_has_been_burned { "burn-another-btn" } else { "burn-btn" };

        div()
            .id(SharedString::from(button_id))
            .w(gpui::px(150.0))
            .h(gpui::px(70.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(success_color)
            .text_color(gpui::white())
            .text_lg()
            .rounded_md()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_center()
            .cursor_pointer()
            .hover(|s| s.bg(success_hover))
            .on_click(cx.listener(move |this, _event, window, cx| {
                println!("Burn clicked!");
                this.burn_existing_iso(window, cx);
            }))
            .child(button_text)
    }

    /// Render Convert & Burn button
    fn render_convert_burn_button(
        &self,
        has_folders: bool,
        success_color: gpui::Hsla,
        success_hover: gpui::Hsla,
        text_muted: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(SharedString::from("convert-burn-btn"))
            .w(gpui::px(150.0))
            .h(gpui::px(70.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(if has_folders { success_color } else { text_muted })
            .text_color(gpui::white())
            .text_lg()
            .rounded_md()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_center()
            .when(has_folders, |el| {
                el.cursor_pointer().hover(|s| s.bg(success_hover))
            })
            .on_click(cx.listener(move |this, _event, window, cx| {
                if has_folders {
                    println!("Convert & Burn clicked!");
                    this.run_conversion(window, cx);
                }
            }))
            .child("Convert\n& Burn")
    }

    /// Cancel any ongoing conversion
    ///
    /// This sets the cancellation flag which will stop new files from being processed.
    /// Files that are currently being converted will finish, but no new files will start.
    /// Returns true if there was a conversion to cancel.
    #[allow(dead_code)]
    pub fn cancel_conversion(&mut self) -> bool {
        if self.conversion_state.is_converting() {
            println!("Cancelling conversion...");
            self.conversion_state.request_cancel();
            true
        } else {
            false
        }
    }

    /// Run burn process - waits for background encoding, creates ISO, then burns
    ///
    /// This simplified version relies on background encoding to convert folders.
    /// If conversion isn't complete, it waits for it to finish before burning.
    fn run_conversion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Don't start if already converting/burning
        if self.conversion_state.is_converting() {
            println!("Already in progress");
            return;
        }

        // Check if we have folders to burn
        if self.folders.is_empty() {
            println!("No folders to burn");
            return;
        }

        // Check if background encoder is available
        if self.background_encoder.is_none() {
            eprintln!("Background encoder not available - cannot burn");
            return;
        }

        println!("Starting burn process...");

        // Get conversion state info
        let all_converted = self.all_folders_converted();
        let total_folders = self.folders.len();

        // Reset conversion state for progress tracking
        self.conversion_state.reset(total_folders);

        if all_converted {
            // All folders already converted - go straight to ISO/burn
            println!("All {} folders already converted", total_folders);
            self.conversion_state.set_stage(BurnStage::CreatingIso);
        } else {
            // Still converting - show waiting stage
            println!("Waiting for background conversion to complete...");
            self.conversion_state.set_stage(BurnStage::Converting);
        }

        let state = self.conversion_state.clone();
        let simulate_burn = cx.global::<AppSettings>().simulate_burn;

        // Get output manager for ISO creation
        let output_manager = match &self.output_manager {
            Some(om) => om.clone(),
            None => {
                eprintln!("No output manager available");
                return;
            }
        };

        // Get encoder handle to check completion
        let encoder_handle = self.background_encoder.as_ref().unwrap().clone();

        // Spawn background thread to wait for conversion and burn
        std::thread::spawn(move || {
            // Wait for all folders to be converted
            loop {
                if state.is_cancelled() {
                    println!("Burn cancelled while waiting for conversion");
                    state.set_stage(BurnStage::Cancelled);
                    state.finish();
                    return;
                }

                // Check if all folders are converted
                let encoder_state = encoder_handle.get_state();
                let guard = encoder_state.lock().unwrap();
                let completed_count = guard.completed.len();
                let queue_empty = guard.queue.is_empty();
                let active_none = guard.active.is_none();
                drop(guard);

                // Update progress
                state.completed.store(completed_count, Ordering::SeqCst);

                if queue_empty && active_none {
                    println!("All folders converted ({} total)", completed_count);
                    break;
                }

                std::thread::sleep(std::time::Duration::from_millis(200));
            }

            // Create ISO
            state.set_stage(BurnStage::CreatingIso);
            println!("\n=== Creating ISO image ===");

            let staging_dir = output_manager.staging_dir();
            let volume_label = "MP3CD".to_string();

            match create_iso(&staging_dir, &volume_label) {
                Ok(result) => {
                    println!("ISO created at: {}", result.iso_path.display());
                    *state.iso_path.lock().unwrap() = Some(result.iso_path.clone());

                    // Coordinate the burn process
                    let config = BurnConfig {
                        simulate: simulate_burn,
                        ..Default::default()
                    };

                    let burn_result = coordinate_burn(&result.iso_path, &state, &config);
                    println!("Burn coordination result: {:?}", burn_result);
                    state.finish();
                }
                Err(e) => {
                    eprintln!("ISO creation failed: {}", e);
                    state.set_stage(BurnStage::Complete);
                    state.finish();
                }
            }
        });

        // Start polling for progress updates
        let window_handle = window.window_handle();
        Self::start_progress_polling(self.conversion_state.clone(), window_handle, cx);

        println!("Burn process started");
        cx.notify();
    }

    /// Burn an existing ISO (for "Burn Another" functionality)
    ///
    /// This skips the conversion step and directly burns the existing ISO.
    fn burn_existing_iso(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Get the ISO path from iso_state
        let iso_path = match &self.iso_state {
            Some(iso) if iso.file_exists() => iso.path.clone(),
            _ => {
                eprintln!("No valid ISO available for burning");
                return;
            }
        };

        // Don't start if already converting/burning
        if self.conversion_state.is_converting() {
            println!("Already burning");
            return;
        }

        println!("Burning existing ISO: {:?}", iso_path);

        // Reset state for burning only (no file conversion)
        self.conversion_state.reset(0);

        let state = self.conversion_state.clone();
        let simulate_burn = cx.global::<AppSettings>().simulate_burn;

        // Spawn background thread for burn coordination
        std::thread::spawn(move || {
            let config = BurnConfig {
                simulate: simulate_burn,
                ..Default::default()
            };

            let result = coordinate_burn(&iso_path, &state, &config);
            println!("Burn coordination result: {:?}", result);
            state.finish();
        });

        // Start polling for progress updates
        let window_handle = window.window_handle();
        Self::start_progress_polling(self.conversion_state.clone(), window_handle, cx);

        println!("Burn Another started");
        cx.notify();
    }

    /// Start a polling loop that updates the UI periodically during conversion
    fn start_progress_polling(
        state: ConversionState,
        window_handle: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        // state is already cloned - no need to read entity

        // Clone in sync part BEFORE the async block - key to avoiding lifetime issues
        cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone(); // Clone here, in sync context
            async move {
                // Poll until conversion finishes
                loop {
                    // Clone BEFORE the await
                    let cx_for_after_await = async_cx.clone();

                    // Wait 50ms between UI updates for smooth progress
                    Timer::after(std::time::Duration::from_millis(50)).await;

                    // Check if we should continue
                    if !state.is_converting() {
                        break;
                    }

                    // Refresh all windows to show updated progress
                    let _ = cx_for_after_await.refresh();

                    // Use this clone for next iteration
                    async_cx = cx_for_after_await;
                }

                // Final refresh to show completion state
                let _ = async_cx.refresh();

                // Save iso_state as soon as ISO is available (for "Burn Another" functionality)
                // This happens even if burn is cancelled - ISO is still usable
                let iso_path = state.iso_path.lock().unwrap().clone();
                if let Some(path) = iso_path {
                    let _ = this.update(&mut async_cx, |folder_list, _cx| {
                        // Only update if we don't already have this ISO saved
                        let should_update = match &folder_list.iso_state {
                            Some(existing) => existing.path != path,
                            None => true,
                        };
                        if should_update {
                            if let Ok(iso_state) = IsoState::new(path, &folder_list.folders) {
                                folder_list.iso_state = Some(iso_state);
                                println!("ISO state saved - ready for Burn/Burn Another");
                            }
                        }
                    });
                }

                // Show success dialog if completed (not cancelled)
                let final_stage = state.get_stage();
                if final_stage == BurnStage::Complete {
                    // Mark that the ISO has been burned (for "Burn Another" button text)
                    let _ = this.update(&mut async_cx, |folder_list, _cx| {
                        folder_list.iso_has_been_burned = true;
                    });

                    let _ = async_cx.update_window(window_handle, |_, window, cx| {
                        let _ = window.prompt(
                            PromptLevel::Info,
                            "Burn Complete",
                            Some("The CD has been burned successfully."),
                            &["OK"],
                            cx,
                        );
                    });
                }
            }
        }).detach();
    }

    /// Start a polling loop for background encoder events
    ///
    /// This polls the encoder event channel and updates folder conversion status.
    /// When all folders are encoded, it triggers automatic ISO generation.
    fn start_encoder_event_polling(cx: &mut Context<Self>) {
        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                loop {
                    let cx_for_after_await = async_cx.clone();

                    // Wait 100ms between updates (encoder events don't need to be as responsive)
                    Timer::after(std::time::Duration::from_millis(100)).await;

                    // Poll encoder events and check if ISO should be generated
                    let should_continue = this
                        .update(&mut async_cx, |this, cx| {
                            // Poll any encoder events
                            let had_events = this.poll_encoder_events();

                            // Check for debounced bitrate recalculation
                            this.check_debounced_bitrate_recalculation();

                            // Check if we should auto-generate ISO
                            if this.maybe_generate_iso(cx) {
                                // ISO generation was triggered
                                println!("Auto-ISO generation triggered");
                            }

                            // Refresh UI if we had events
                            if had_events {
                                cx.notify();
                            }

                            // Continue polling as long as we have a background encoder
                            this.background_encoder.is_some()
                        })
                        .unwrap_or(false);

                    if !should_continue {
                        break;
                    }

                    // Refresh UI
                    let _ = cx_for_after_await.refresh();
                    async_cx = cx_for_after_await;
                }
            }
        })
        .detach();
    }

    /// Start a polling loop that drains imported folders and updates the UI
    fn start_import_polling(state: ImportState, cx: &mut Context<Self>) {
        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                loop {
                    let cx_for_after_await = async_cx.clone();

                    // Wait 50ms between updates
                    Timer::after(std::time::Duration::from_millis(50)).await;

                    // Drain any scanned folders and add to the list
                    let folders = state.drain_folders();
                    if !folders.is_empty() {
                        let _ = this.update(&mut async_cx, |this, _cx| {
                            for folder in folders {
                                // Queue for background encoding if available
                                this.queue_folder_for_encoding(&folder);
                                this.folders.push(folder);
                            }
                            // Invalidate ISO since folder list changed
                            this.iso_state = None;
                            this.iso_generation_attempted = false;
                            this.iso_has_been_burned = false;
                            // Record change time for debounced bitrate recalculation
                            this.last_folder_change = Some(std::time::Instant::now());
                        });
                    }

                    // Check if we should continue
                    if !state.is_importing() {
                        break;
                    }

                    // Refresh UI
                    let _ = cx_for_after_await.refresh();

                    async_cx = cx_for_after_await;
                }

                // Final drain and refresh
                let folders = state.drain_folders();
                if !folders.is_empty() {
                    let _ = this.update(&mut async_cx, |this, _cx| {
                        for folder in folders {
                            // Queue for background encoding if available
                            this.queue_folder_for_encoding(&folder);
                            this.folders.push(folder);
                        }
                        // Invalidate ISO since folder list changed
                        this.iso_state = None;
                        this.iso_generation_attempted = false;
                        this.iso_has_been_burned = false;
                        // Record change time for debounced bitrate recalculation
                        this.last_folder_change = Some(std::time::Instant::now());
                    });
                }
                let _ = async_cx.refresh();
            }
        }).detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Helper to create a test MusicFolder
    fn test_folder(path: &str) -> MusicFolder {
        use crate::core::{FolderConversionStatus, FolderId};
        MusicFolder {
            id: FolderId::from_path(std::path::Path::new(path)),
            path: PathBuf::from(path),
            file_count: 10,
            total_size: 50_000_000,
            total_duration: 2400.0, // 40 minutes
            album_art: None,
            audio_files: Vec::new(),
            conversion_status: FolderConversionStatus::default(),
        }
    }

    #[test]
    fn test_folder_list_new() {
        let list = FolderList::new_for_test();
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_add_folder() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/folder1"));
        list.folders.push(test_folder("/test/folder2"));

        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_remove_folder() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/folder1"));
        list.folders.push(test_folder("/test/folder2"));

        list.remove_folder(0);

        assert_eq!(list.len(), 1);
        assert_eq!(list.folders[0].path, PathBuf::from("/test/folder2"));
    }

    #[test]
    fn test_move_folder_forward() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/a"));
        list.folders.push(test_folder("/test/b"));
        list.folders.push(test_folder("/test/c"));

        // Move "a" to position 2 (after "b")
        list.move_folder(0, 2);

        assert_eq!(list.folders[0].path, PathBuf::from("/test/b"));
        assert_eq!(list.folders[1].path, PathBuf::from("/test/a"));
        assert_eq!(list.folders[2].path, PathBuf::from("/test/c"));
    }

    #[test]
    fn test_move_folder_backward() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/a"));
        list.folders.push(test_folder("/test/b"));
        list.folders.push(test_folder("/test/c"));

        // Move "c" to position 0 (before "a")
        list.move_folder(2, 0);

        assert_eq!(list.folders[0].path, PathBuf::from("/test/c"));
        assert_eq!(list.folders[1].path, PathBuf::from("/test/a"));
        assert_eq!(list.folders[2].path, PathBuf::from("/test/b"));
    }

    #[test]
    fn test_clear() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/folder1"));
        list.folders.push(test_folder("/test/folder2"));

        list.clear();

        assert!(list.is_empty());
    }

    #[test]
    fn test_total_files() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/folder1")); // 10 files
        list.folders.push(test_folder("/test/folder2")); // 10 files

        assert_eq!(list.total_files(), 20);
    }

    #[test]
    fn test_total_size() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/folder1")); // 50MB
        list.folders.push(test_folder("/test/folder2")); // 50MB

        assert_eq!(list.total_size(), 100_000_000);
    }

    // ConversionState tests

    #[test]
    fn test_conversion_state_new() {
        let state = ConversionState::new();

        assert!(!state.is_converting());
        let (completed, failed, total) = state.progress();
        assert_eq!(completed, 0);
        assert_eq!(failed, 0);
        assert_eq!(total, 0);
    }

    #[test]
    fn test_conversion_state_reset() {
        let state = ConversionState::new();

        state.reset(24);

        assert!(state.is_converting());
        let (completed, failed, total) = state.progress();
        assert_eq!(completed, 0);
        assert_eq!(failed, 0);
        assert_eq!(total, 24);
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
    fn test_conversion_state_progress_updates() {
        let state = ConversionState::new();
        state.reset(5);

        // Simulate completing some files
        state.completed.fetch_add(1, Ordering::SeqCst);
        state.completed.fetch_add(1, Ordering::SeqCst);
        state.failed.fetch_add(1, Ordering::SeqCst);

        let (completed, failed, total) = state.progress();
        assert_eq!(completed, 2);
        assert_eq!(failed, 1);
        assert_eq!(total, 5);
    }

    #[test]
    fn test_conversion_state_clone_shares_atomics() {
        let state1 = ConversionState::new();
        state1.reset(10);

        let state2 = state1.clone();

        // Update via state1
        state1.completed.fetch_add(5, Ordering::SeqCst);

        // Should be visible via state2 (shared Arc)
        let (completed, _, _) = state2.progress();
        assert_eq!(completed, 5);
    }

    #[test]
    fn test_conversion_state_thread_safety() {
        use std::thread;

        let state = ConversionState::new();
        state.reset(100);

        let mut handles = vec![];

        // Spawn 10 threads, each incrementing completed 10 times
        for _ in 0..10 {
            let state_clone = state.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..10 {
                    state_clone.completed.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let (completed, _, _) = state.progress();
        assert_eq!(completed, 100);
    }

    #[test]
    fn test_conversion_state_cancellation() {
        let state = ConversionState::new();
        state.reset(10);

        // Initially not cancelled
        assert!(!state.is_cancelled());

        // Request cancel
        state.request_cancel();

        // Should now be cancelled
        assert!(state.is_cancelled());
        // But should still be converting (in-flight tasks finish)
        assert!(state.is_converting());
    }

    #[test]
    fn test_conversion_state_reset_clears_cancel() {
        let state = ConversionState::new();
        state.reset(10);
        state.request_cancel();
        assert!(state.is_cancelled());

        // Reset should clear the cancel flag
        state.reset(5);
        assert!(!state.is_cancelled());
        assert!(state.is_converting());
    }
}
