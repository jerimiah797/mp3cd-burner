//! FolderList component - The main application view with folder list
//!
//! This is currently the root view of the application, containing:
//! - Header
//! - Folder list with drag-and-drop
//! - Status bar

use gpui::{div, prelude::*, rgb, AnyWindowHandle, AsyncApp, Context, ExternalPaths, FocusHandle, IntoElement, PathPromptOptions, PromptLevel, Render, ScrollHandle, SharedString, Timer, WeakEntity, Window};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crate::actions::{NewProfile, OpenProfile, SaveProfile, SetVolumeLabel, take_pending_files};
use super::VolumeLabelDialog;

use super::folder_item::{render_folder_item, DraggedFolder, FolderItemProps};
use super::status_bar::{
    is_stage_cancelable, render_burn_button_base, render_convert_burn_button_base,
    render_erase_burn_button_base, render_import_progress, render_iso_too_large,
    render_progress_box, render_stats_panel, StatusBarState,
};
use crate::burning::{determine_iso_action, IsoAction, IsoState};
use crate::conversion::{calculate_multipass_bitrate, BackgroundEncoderHandle, OutputManager};
use crate::core::{
    find_album_folders, scan_music_folder,
    AppSettings, BurnStage, ConversionState, DisplaySettings, FolderConversionStatus, FolderId, ImportState, MusicFolder, WindowState,
};
use crate::profiles::ProfileLoadSetup;
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
    /// Whether we've subscribed to bounds changes (for saving window state)
    bounds_subscription_set: bool,
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
    /// Flag to clear folders after save completes (for New -> Save flow)
    pending_new_after_save: bool,
    /// Flag to show open file picker after save completes (for Open -> Save flow)
    pending_open_after_save: bool,
    /// Pending profile load setup (for async profile loading)
    pending_profile_load: Option<ProfileLoadSetup>,
    /// CD volume label (for ISO creation)
    volume_label: String,
    /// Receiver for volume label updates from the dialog
    pending_volume_label_rx: Option<std::sync::mpsc::Receiver<String>>,
    /// Pending burn action to trigger after volume label dialog closes
    pending_burn_action: Option<PendingBurnAction>,
    /// Path to the currently saved profile (None if never saved)
    current_profile_path: Option<PathBuf>,
    /// Whether there are unsaved changes since last save/load
    has_unsaved_changes: bool,
}

/// Action to take after volume label dialog closes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingBurnAction {
    /// Burn existing ISO
    BurnExisting,
    /// Run conversion then burn
    ConvertAndBurn,
}

impl FolderList {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
            appearance_subscription_set: false,
            bounds_subscription_set: false,
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
            pending_new_after_save: false,
            pending_open_after_save: false,
            pending_profile_load: None,
            volume_label: "Untitled MP3CD".to_string(),
            pending_volume_label_rx: None,
            pending_burn_action: None,
            current_profile_path: None,
            has_unsaved_changes: false,
        }
    }

    /// Create a new FolderList for testing (without GPUI context)
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
            appearance_subscription_set: false,
            bounds_subscription_set: false,
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
            pending_new_after_save: false,
            pending_open_after_save: false,
            pending_profile_load: None,
            volume_label: "Untitled MP3CD".to_string(),
            pending_volume_label_rx: None,
            pending_burn_action: None,
            current_profile_path: None,
            has_unsaved_changes: false,
        }
    }

    /// Initialize the background encoder for immediate folder conversion
    ///
    /// This should be called after construction when background encoding is desired.
    /// If not called, folders will only be converted when "Burn" is clicked (legacy mode).
    /// Returns a clone of the encoder handle so it can be stored as a global.
    pub fn enable_background_encoding(&mut self) -> Result<BackgroundEncoderHandle, String> {
        use crate::conversion::BackgroundEncoder;

        // Create the background encoder (this spawns its own thread with Tokio runtime)
        // IMPORTANT: Use the output_manager returned by the encoder - it's the same one
        // used for encoding, so ISO staging will find the encoded files!
        let (_encoder, handle, event_rx, output_manager) = BackgroundEncoder::new()?;

        // Clean up old sessions from previous runs
        output_manager.cleanup_old_sessions()?;

        // Store the handle, event receiver, and output manager
        let handle_clone = handle.clone();
        self.background_encoder = Some(handle);
        self.encoder_event_rx = Some(event_rx);
        self.output_manager = Some(output_manager);

        println!("Background encoding enabled, session: {:?}",
            self.output_manager.as_ref().map(|m| m.session_id()));

        Ok(handle_clone)
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

    /// Update encoder's embed_album_art setting
    pub fn set_embed_album_art(&self, embed: bool) {
        if let Some(ref encoder) = self.background_encoder {
            println!("[FolderList] Sending embed_album_art={} to encoder", embed);
            encoder.set_embed_album_art(embed);
        } else {
            println!("[FolderList] WARNING: No encoder to send embed_album_art to!");
        }
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

            // Check if active (supports multiple active folders)
            if guard.active.contains_key(folder_id) {
                // Get actual progress from state
                let (files_completed, files_total) = guard.active_progress
                    .get(folder_id)
                    .copied()
                    .unwrap_or((0, 0));
                return FolderConversionStatus::Converting {
                    files_completed,
                    files_total,
                };
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
                // Check both encoder's completed map AND folder's own conversion status
                // (folders loaded from bundles have Converted status but aren't in encoder)
                guard.completed.contains_key(&folder.id)
                    || matches!(folder.conversion_status, FolderConversionStatus::Converted { .. })
            })
        } else {
            // In legacy mode, check folder status directly
            self.folders.iter().all(|folder| {
                matches!(folder.conversion_status, FolderConversionStatus::Converted { .. })
            })
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
                    // Initialize progress for the new folder (0 completed out of total)
                    if let Some(ref encoder) = self.background_encoder {
                        let state = encoder.get_state();
                        let mut guard = state.lock().unwrap();
                        guard.active_progress.insert(id.clone(), (0, files_total));
                    }
                    println!("Encoding started: {:?} ({} files)", id, files_total);
                }
                EncoderEvent::FolderProgress { id, files_completed, files_total } => {
                    // Update progress in encoder state for UI rendering
                    if let Some(ref encoder) = self.background_encoder {
                        let state = encoder.get_state();
                        let mut guard = state.lock().unwrap();
                        guard.active_progress.insert(id.clone(), (files_completed, files_total));
                    }
                    println!("Encoding progress: {:?} {}/{}", id, files_completed, files_total);
                }
                EncoderEvent::FolderCompleted { id, output_dir, output_size, lossless_bitrate } => {
                    println!("Encoding complete: {:?} -> {:?} ({} bytes, bitrate: {:?})",
                        id, output_dir, output_size, lossless_bitrate);

                    // Clear active progress for this folder
                    if let Some(ref encoder) = self.background_encoder {
                        let state = encoder.get_state();
                        let mut guard = state.lock().unwrap();
                        guard.active_progress.remove(&id);
                        guard.active.remove(&id);
                    }

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

    /// Poll for volume label updates from the dialog
    ///
    /// Returns true if a label was received (useful for knowing if UI needs refresh)
    fn poll_volume_label(&mut self) -> bool {
        if let Some(ref rx) = self.pending_volume_label_rx {
            if let Ok(label) = rx.try_recv() {
                println!("Volume label set to: {}", label);

                // If the label changed, invalidate the ISO so it gets regenerated
                if self.volume_label != label {
                    if self.iso_state.is_some() {
                        println!("Volume label changed - invalidating existing ISO");
                        self.iso_state = None;
                        self.iso_generation_attempted = false;
                    }
                }

                self.volume_label = label;
                // Clear the receiver since we got the label
                self.pending_volume_label_rx = None;
                // Note: pending_burn_action will be handled in check_pending_burn_action
                return true;
            }
        }
        false
    }

    /// Check for files opened via Finder and load them
    ///
    /// This should be called from the render loop. When a user double-clicks
    /// a .mp3cd file in Finder, macOS opens our app with that file. The path
    /// is stored in a static and we poll for it here.
    fn poll_pending_open_files(&mut self, cx: &mut Context<Self>) {
        let pending_paths = take_pending_files();
        for path in pending_paths {
            println!("Loading profile from Finder: {:?}", path);
            // Don't prompt to save - just load the profile directly
            // (this is the expected behavior when double-clicking a file)
            if let Err(e) = self.load_profile(&path, cx) {
                eprintln!("Failed to load profile: {}", e);
            }
        }
    }

    /// Check and execute any pending burn action
    ///
    /// This should be called from the render loop where we have window access.
    /// Returns true if an action was triggered.
    fn check_pending_burn_action(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Only trigger if we've finished receiving the volume label
        // (pending_volume_label_rx is None means dialog closed and we got the label)
        if self.pending_volume_label_rx.is_some() {
            return false;
        }

        // Check what action is pending (without taking it yet)
        let action = match self.pending_burn_action {
            Some(action) => action,
            None => return false,
        };

        match action {
            PendingBurnAction::BurnExisting => {
                // Wait for ISO to be ready before triggering burn
                if self.iso_state.is_none() {
                    // ISO is still being regenerated, wait for next cycle
                    return false;
                }
                // ISO is ready, take the action and burn
                self.pending_burn_action = None;
                println!("Triggering burn after volume label dialog");
                self.burn_existing_iso(window, cx);
            }
            PendingBurnAction::ConvertAndBurn => {
                // run_conversion handles waiting for ISO, so trigger immediately
                self.pending_burn_action = None;
                println!("Triggering convert & burn after volume label dialog");
                self.run_conversion(window, cx);
            }
        }
        true
    }

    /// Check if ISO needs to be generated and do it if ready
    ///
    /// This should be called periodically to auto-generate ISO when all folders are encoded.
    /// Returns true if ISO generation was triggered.
    fn maybe_generate_iso(&mut self, cx: &mut Context<Self>) -> bool {
        use crate::burning::IsoGenerationCheck;

        // Check all conditions for ISO generation
        let check = IsoGenerationCheck {
            has_valid_iso: self.can_burn_another(),
            already_attempted: self.iso_generation_attempted,
            has_folders: !self.folders.is_empty(),
            all_converted: self.all_folders_converted(),
            is_busy: self.conversion_state.is_converting(),
        };

        if !check.should_generate() {
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

        // Spawn ISO generation in background
        let folders: Vec<_> = self.folders.iter().cloned().collect();
        crate::burning::spawn_iso_generation(
            output_manager,
            folders.clone(),
            self.conversion_state.clone(),
            self.volume_label.clone(),
        );

        // Start polling for ISO creation progress
        Self::start_iso_creation_polling(self.conversion_state.clone(), folders, cx);

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

        // Notify encoder that import is starting (delays encoding until complete)
        if let Some(ref encoder) = self.background_encoder {
            encoder.import_started();
        }

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
                // Mark as having unsaved changes
                self.has_unsaved_changes = true;
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
            // Mark as having unsaved changes
            self.has_unsaved_changes = true;
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
            // Mark as having unsaved changes
            self.has_unsaved_changes = true;
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

    /// Save the current state as a profile to the specified path
    ///
    /// If `for_bundle` is true, saves as v2.0 bundle format including converted files.
    /// If `for_bundle` is false, saves as legacy format (metadata only).
    pub fn save_profile(&mut self, path: &std::path::Path, profile_name: String, for_bundle: bool) -> Result<(), String> {
        // Save volume_label if it's not the default
        let volume_label = if self.volume_label == super::DEFAULT_LABEL {
            None
        } else {
            Some(self.volume_label.clone())
        };

        // If saving as bundle and we have converted files in temp, copy them to bundle first
        if for_bundle {
            if let Some(output_manager) = &self.output_manager {
                // Collect folder IDs that have been converted
                let converted_folder_ids: Vec<crate::core::FolderId> = self.folders.iter()
                    .filter_map(|f| {
                        if matches!(f.conversion_status, crate::core::FolderConversionStatus::Converted { .. }) {
                            Some(f.id.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                if !converted_folder_ids.is_empty() && !output_manager.is_bundle_mode() {
                    // Copy converted files from temp to bundle
                    output_manager.copy_to_bundle(path, &converted_folder_ids)?;
                }
            }
        }

        // Save the profile
        crate::profiles::save_profile_to_path(
            path,
            profile_name,
            &self.folders,
            self.output_manager.as_ref(),
            self.iso_state.as_ref(),
            volume_label,
            for_bundle,
        )?;

        // If we saved as bundle, update output manager to use the bundle path for future encodes
        if for_bundle {
            if let Some(output_manager) = &mut self.output_manager {
                output_manager.set_bundle_path(Some(path.to_path_buf()));
            }
        }

        Ok(())
    }

    /// Check if any folders have been converted
    fn has_converted_folders(&self) -> bool {
        self.folders.iter().any(|f| matches!(f.conversion_status, crate::core::FolderConversionStatus::Converted { .. }))
    }

    /// Load a profile and restore its state
    ///
    /// This will:
    /// 1. Load the profile metadata from disk (fast)
    /// 2. Validate the saved conversion state (fast)
    /// 3. Scan folders asynchronously (background thread)
    /// 4. Restore conversion status for valid folders
    /// 5. Queue folders needing re-encoding to the background encoder
    pub fn load_profile(&mut self, path: &std::path::Path, cx: &mut Context<Self>) -> Result<(), String> {
        // Don't start if already importing
        if self.import_state.is_importing() {
            return Err("Import already in progress".to_string());
        }

        // Prepare the profile load (fast - just reads metadata)
        let setup = crate::profiles::prepare_profile_load(path)?;

        let folder_count = setup.folder_paths.len();
        if folder_count == 0 {
            return Err("Profile has no folders".to_string());
        }

        println!("Starting async profile load of {} folders", folder_count);

        // Clear current state
        self.folders.clear();
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.iso_has_been_burned = false;

        // Remember the profile path and mark as saved (no unsaved changes after load)
        self.current_profile_path = Some(path.to_path_buf());
        self.has_unsaved_changes = false;

        // Restore volume label from profile (or default if not saved)
        self.volume_label = setup.volume_label.clone().unwrap_or_else(|| super::DEFAULT_LABEL.to_string());

        // If loading a bundle, set up the output manager to use the bundle path
        if let Some(ref bundle_path) = setup.bundle_path {
            if let Some(output_manager) = &mut self.output_manager {
                output_manager.set_bundle_path(Some(bundle_path.clone()));
                println!("Set output manager bundle path to: {:?}", bundle_path);
            }
        } else {
            // Legacy format - clear any bundle path
            if let Some(output_manager) = &mut self.output_manager {
                output_manager.set_bundle_path(None);
            }
        }

        // Clear the encoder state and delete converted files
        // (Don't delete for bundle - we're using the bundle's converted files!)
        if setup.bundle_path.is_none() {
            if let Some(encoder) = &self.background_encoder {
                encoder.clear_all();
            }
        }

        // Store the setup for the polling callback
        self.pending_profile_load = Some(setup.clone());

        // Reset import state
        self.import_state.reset(folder_count);

        // Notify encoder that import is starting (delays encoding until complete)
        if let Some(ref encoder) = self.background_encoder {
            encoder.import_started();
        }

        // Clone state for background thread
        let state = self.import_state.clone();
        let folder_paths = setup.folder_paths.clone();

        // Spawn background thread for scanning
        std::thread::spawn(move || {
            for path in folder_paths {
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
            println!("Profile import complete");
        });

        // Start polling for results (profile-aware)
        Self::start_profile_import_polling(self.import_state.clone(), cx);

        cx.notify();
        Ok(())
    }

    /// Clear current state for a new profile (called from File > New menu)
    ///
    /// If there are unsaved folders, shows a confirmation dialog first.
    pub fn new_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // If no unsaved changes, just clear immediately
        if !self.has_unsaved_changes {
            self.clear_for_new_profile(cx);
            return;
        }

        // Show confirmation dialog
        let receiver = window.prompt(
            PromptLevel::Warning,
            "Unsaved Changes",
            Some("You have folders that haven't been saved to a Burn Profile. What would you like to do?"),
            &["Save Burn Profile...", "Don't Save", "Cancel"],
            cx,
        );

        let window_handle = window.window_handle();

        cx.spawn(move |this_handle: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                if let Ok(choice) = receiver.await {
                    match choice {
                        0 => {
                            // Save - show save dialog, then clear
                            println!("User chose to save - showing save dialog");
                            let _ = async_cx.update_window(window_handle, |_, window, cx| {
                                let _ = this_handle.update(cx, |this, cx| {
                                    // Set flag to clear after save
                                    this.pending_new_after_save = true;
                                    this.save_profile_dialog(window, cx);
                                });
                            });
                        }
                        1 => {
                            // Don't Save - clear immediately
                            println!("User chose not to save - clearing");
                            let _ = async_cx.update(|cx| {
                                let _ = this_handle.update(cx, |this, cx| {
                                    this.clear_for_new_profile(cx);
                                });
                            });
                        }
                        2 => {
                            // Cancel - do nothing
                            println!("User cancelled new profile");
                        }
                        _ => {}
                    }
                }
            }
        }).detach();
    }

    /// Actually clear the state for a new profile
    fn clear_for_new_profile(&mut self, cx: &mut Context<Self>) {
        self.folders.clear();
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.iso_has_been_burned = false;
        self.last_folder_change = None;
        self.last_calculated_bitrate = None;
        self.volume_label = super::DEFAULT_LABEL.to_string();
        self.current_profile_path = None;
        self.has_unsaved_changes = false;
        // Clear the encoder state and delete converted files
        if let Some(encoder) = &self.background_encoder {
            encoder.clear_all();
        }
        println!("New profile - cleared all folders and encoder state");
        cx.notify();
    }

    /// Show the volume label dialog
    ///
    /// Opens a modal dialog for editing the CD volume label.
    /// If `pending_action` is provided, that action will be triggered after the dialog closes.
    fn show_volume_label_dialog(&mut self, pending_action: Option<PendingBurnAction>, cx: &mut Context<Self>) {
        let current_label = self.volume_label.clone();

        // Store the pending action to trigger after dialog closes
        self.pending_burn_action = pending_action;

        // Create a channel for the dialog to send the label back
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending_volume_label_rx = Some(rx);

        let _dialog_handle = VolumeLabelDialog::open(cx, Some(current_label), move |label| {
            // Send the label through the channel - this avoids RefCell borrow conflicts
            // since we're not trying to update GPUI state directly in the callback
            let _ = tx.send(label);
        });
    }

    /// Show file picker to open a profile (called from File > Open menu)
    ///
    /// If there are unsaved changes, shows a confirmation dialog first.
    pub fn open_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // If no unsaved changes, just show file picker immediately
        if !self.has_unsaved_changes {
            self.show_open_file_picker(cx);
            return;
        }

        // Show confirmation dialog
        let receiver = window.prompt(
            PromptLevel::Warning,
            "Unsaved Changes",
            Some("You have folders that haven't been saved to a Burn Profile. What would you like to do?"),
            &["Save Burn Profile...", "Don't Save", "Cancel"],
            cx,
        );

        let window_handle = window.window_handle();

        cx.spawn(move |this_handle: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                if let Ok(choice) = receiver.await {
                    match choice {
                        0 => {
                            // Save first, then open
                            println!("User chose to save before opening - showing save dialog");
                            let _ = async_cx.update_window(window_handle, |_, window, cx| {
                                let _ = this_handle.update(cx, |this, cx| {
                                    // Set flag to show open picker after save completes
                                    this.pending_open_after_save = true;
                                    this.save_profile_dialog(window, cx);
                                });
                            });
                        }
                        1 => {
                            // Don't Save - open profile directly
                            println!("User chose not to save - opening profile");
                            let _ = async_cx.update(|cx| {
                                let _ = this_handle.update(cx, |this, cx| {
                                    this.show_open_file_picker(cx);
                                });
                            });
                        }
                        2 => {
                            // Cancel - do nothing
                            println!("User cancelled open profile");
                        }
                        _ => {}
                    }
                }
            }
        }).detach();
    }

    /// Actually show the file picker to open a profile
    fn show_open_file_picker(&mut self, cx: &mut Context<Self>) {
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
    ///
    /// If there are converted folders, shows a dialog asking whether to include audio files.
    /// Then shows the file picker and saves the profile.
    pub fn save_profile_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.folders.is_empty() {
            println!("No folders to save");
            return;
        }

        // Check if we have converted folders - if so, offer bundle option
        if self.has_converted_folders() {
            // Show dialog to choose save format
            let receiver = window.prompt(
                PromptLevel::Info,
                "Save Options",
                Some("Some folders have already been converted. Would you like to include the converted audio files in the profile?"),
                &["Include Audio Files", "Metadata Only", "Cancel"],
                cx,
            );

            let window_handle = window.window_handle();

            cx.spawn(move |this_handle: WeakEntity<Self>, cx: &mut AsyncApp| {
                let mut async_cx = cx.clone();
                async move {
                    if let Ok(choice) = receiver.await {
                        match choice {
                            0 => {
                                // Include audio - save as bundle
                                let _ = async_cx.update_window(window_handle, |_, window, cx| {
                                    let _ = this_handle.update(cx, |this, cx| {
                                        this.show_save_file_picker(window, cx, true);
                                    });
                                });
                            }
                            1 => {
                                // Metadata only - save as legacy
                                let _ = async_cx.update_window(window_handle, |_, window, cx| {
                                    let _ = this_handle.update(cx, |this, cx| {
                                        this.show_save_file_picker(window, cx, false);
                                    });
                                });
                            }
                            _ => {
                                // Cancel - reset flags
                                let _ = async_cx.update(|cx| {
                                    let _ = this_handle.update(cx, |this, _cx| {
                                        this.pending_new_after_save = false;
                                        this.pending_open_after_save = false;
                                    });
                                });
                            }
                        }
                    }
                }
            }).detach();
        } else {
            // No converted folders - just show file picker (save as metadata only)
            self.show_save_file_picker(window, cx, false);
        }
    }

    /// Internal method to show the save file picker
    fn show_save_file_picker(&mut self, _window: &mut Window, cx: &mut Context<Self>, for_bundle: bool) {
        // Use current profile path if we've saved before, otherwise generate from first folder
        let (start_dir, default_filename) = if let Some(ref current_path) = self.current_profile_path {
            // Use the directory and filename from the current profile
            let dir = current_path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| dirs::document_dir().unwrap_or_else(|| PathBuf::from(".")));
            // Get just the file stem (name without any extension) and add .mp3cd
            let stem = current_path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "Untitled".to_string());
            let filename = format!("{}.mp3cd", stem);
            (dir, filename)
        } else {
            // Generate a default filename from the first folder
            let default_name = self.folders.first()
                .and_then(|f| f.path.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Untitled".to_string());
            let dir = dirs::document_dir().unwrap_or_else(|| PathBuf::from("."));
            (dir, format!("{}.mp3cd", default_name))
        };

        // Extract profile name from filename (without .mp3cd extension)
        let profile_name = default_filename
            .strip_suffix(".mp3cd")
            .unwrap_or(&default_filename)
            .to_string();

        let receiver = cx.prompt_for_new_path(&start_dir, Some(&default_filename));
        cx.spawn(move |this_handle: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                match receiver.await {
                    Ok(Ok(Some(path))) => {
                        // Extract profile name from chosen path
                        let chosen_name = path.file_stem()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| profile_name);

                        let _ = this_handle.update(&mut async_cx, |this, cx| {
                            if let Err(e) = this.save_profile(&path, chosen_name, for_bundle) {
                                eprintln!("Failed to save profile: {}", e);
                                this.pending_new_after_save = false;
                                this.pending_open_after_save = false;
                            } else {
                                println!("Profile saved to: {:?} (bundle: {})", path, for_bundle);
                                // Remember the save path and mark as saved
                                this.current_profile_path = Some(path);
                                this.has_unsaved_changes = false;

                                // If we were saving as part of New flow, now clear the folders
                                if this.pending_new_after_save {
                                    this.pending_new_after_save = false;
                                    this.clear_for_new_profile(cx);
                                }
                                // If we were saving as part of Open flow, now show file picker
                                if this.pending_open_after_save {
                                    this.pending_open_after_save = false;
                                    this.show_open_file_picker(cx);
                                }
                            }
                        });
                    }
                    _ => {
                        // Cancelled or error - reset the flags
                        let _ = this_handle.update(&mut async_cx, |this, _cx| {
                            this.pending_new_after_save = false;
                            this.pending_open_after_save = false;
                        });
                    }
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
        // Clone display settings to avoid borrow conflict with cx
        let display_settings = cx.global::<DisplaySettings>().clone();
        let mut list = div().w_full().flex().flex_col().gap_2();

        for (index, folder) in self.folders.iter().enumerate() {
            // Get live conversion status from encoder state (for progress updates)
            let live_status = self.get_folder_conversion_status(&folder.id);
            let mut folder_with_live_status = folder.clone();
            // Only update if actively converting (preserve Converted status from folder)
            if matches!(live_status, FolderConversionStatus::Converting { .. }) {
                folder_with_live_status.conversion_status = live_status;
            }

            let props = FolderItemProps {
                index,
                folder: folder_with_live_status,
                is_drop_target: drop_target == Some(index),
                theme: *theme,
                show_file_count: display_settings.show_file_count,
                show_original_size: display_settings.show_original_size,
                show_converted_size: display_settings.show_converted_size,
                show_source_format: display_settings.show_source_format,
                show_source_bitrate: display_settings.show_source_bitrate,
                show_final_bitrate: display_settings.show_final_bitrate,
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

        // Subscribe to bounds changes to save window state (once)
        if !self.bounds_subscription_set {
            self.bounds_subscription_set = true;
            cx.observe_window_bounds(window, |_this, window, _cx| {
                let bounds = window.bounds();
                let state = WindowState {
                    x: bounds.origin.x.into(),
                    y: bounds.origin.y.into(),
                    width: bounds.size.width.into(),
                    height: bounds.size.height.into(),
                };
                if let Err(e) = state.save() {
                    eprintln!("Failed to save window state: {}", e);
                }
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

        // Check for files opened via Finder (double-click on .mp3cd files)
        self.poll_pending_open_files(cx);

        // Check for pending burn action after volume label dialog closes
        self.check_pending_burn_action(window, cx);

        // Update window title to include volume label
        let title = if self.volume_label == "Untitled MP3CD" || self.volume_label.is_empty() {
            "MP3 CD Burner".to_string()
        } else {
            format!("MP3 CD Burner - {}", self.volume_label)
        };
        window.set_window_title(&title);

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
        let on_new_profile = cx.listener(|this, _: &NewProfile, window, cx| {
            this.new_profile(window, cx);
        });
        let on_open_profile = cx.listener(|this, _: &OpenProfile, window, cx| {
            this.open_profile(window, cx);
        });
        let on_save_profile = cx.listener(|this, _: &SaveProfile, window, cx| {
            this.save_profile_dialog(window, cx);
        });
        let on_set_volume_label = cx.listener(|this, _: &SetVolumeLabel, _window, cx| {
            this.show_volume_label_dialog(None, cx);
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
            .on_action(on_set_volume_label)
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
        let is_cancelable = is_stage_cancelable(state);

        div()
            .id(SharedString::from("convert-progress-container"))
            .flex()
            .flex_col()
            .gap_2()
            .items_center()
            // Progress display (hide when waiting for user to approve erase)
            .when(state.burn_stage != BurnStage::ErasableDiscDetected, |el| {
                let mut progress_box = render_progress_box(state, theme);
                if is_cancelable {
                    progress_box = progress_box
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _event, _window, _cx| {
                            this.conversion_state.request_cancel();
                        }));
                }
                el.child(progress_box)
            })
            // Erase & Burn button (only show when erasable disc detected)
            .when(state.burn_stage == BurnStage::ErasableDiscDetected, |el| {
                el.child(
                    render_erase_burn_button_base(success_color, success_hover)
                        .on_click(cx.listener(|this, _event, _window, _cx| {
                            println!("Erase & Burn clicked");
                            this.conversion_state.erase_approved.store(true, Ordering::SeqCst);
                        })),
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
        render_burn_button_base(iso_has_been_burned, success_color, success_hover)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                println!("Burn clicked - showing volume label dialog");
                this.show_volume_label_dialog(Some(PendingBurnAction::BurnExisting), cx);
            }))
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
        render_convert_burn_button_base(has_folders, success_color, success_hover, text_muted)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if has_folders {
                    println!("Convert & Burn clicked - showing volume label dialog");
                    this.show_volume_label_dialog(Some(PendingBurnAction::ConvertAndBurn), cx);
                }
            }))
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
        let encoder_handle = match &self.background_encoder {
            Some(handle) => handle.clone(),
            None => {
                eprintln!("Background encoder not available - cannot burn");
                return;
            }
        };

        // Get output manager for ISO creation
        let output_manager = match &self.output_manager {
            Some(om) => om.clone(),
            None => {
                eprintln!("No output manager available");
                return;
            }
        };

        println!("Starting burn process...");

        // Get conversion state info
        let all_converted = self.all_folders_converted();
        let total_folders = self.folders.len();

        // Reset conversion state for progress tracking
        self.conversion_state.reset(total_folders);

        if all_converted {
            println!("All {} folders already converted", total_folders);
            self.conversion_state.set_stage(BurnStage::CreatingIso);
        } else {
            println!("Waiting for background conversion to complete...");
            self.conversion_state.set_stage(BurnStage::Converting);
        }

        let state = self.conversion_state.clone();
        let simulate_burn = cx.global::<AppSettings>().simulate_burn;
        let folders: Vec<_> = self.folders.iter().cloned().collect();
        let volume_label = self.volume_label.clone();

        // Spawn background thread to execute the full burn workflow
        std::thread::spawn(move || {
            crate::burning::execute_full_burn(state, encoder_handle, output_manager, folders, simulate_burn, volume_label);
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

        // Spawn background thread for burn execution
        std::thread::spawn(move || {
            crate::burning::execute_burn_existing(state, iso_path, simulate_burn);
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

                            // Poll for volume label updates from the dialog
                            let label_updated = this.poll_volume_label();

                            // Check for debounced bitrate recalculation
                            this.check_debounced_bitrate_recalculation();

                            // Check if we should auto-generate ISO
                            if this.maybe_generate_iso(cx) {
                                // ISO generation was triggered
                                println!("Auto-ISO generation triggered");
                            }

                            // Refresh UI if we had events or label was updated
                            if had_events || label_updated {
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
                            // Mark as having unsaved changes
                            this.has_unsaved_changes = true;
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
                        // Mark as having unsaved changes
                        this.has_unsaved_changes = true;
                        // Record change time for debounced bitrate recalculation
                        this.last_folder_change = Some(std::time::Instant::now());
                    });
                }

                // Notify encoder that import is complete (resumes encoding)
                let _ = this.update(&mut async_cx, |this, _cx| {
                    if let Some(ref encoder) = this.background_encoder {
                        encoder.import_complete();
                    }
                });

                let _ = async_cx.refresh();
            }
        }).detach();
    }

    /// Start a polling loop for profile import that restores conversion status
    ///
    /// This is similar to `start_import_polling` but handles restoration of
    /// conversion status from the saved profile state.
    fn start_profile_import_polling(state: ImportState, cx: &mut Context<Self>) {
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
                            for mut folder in folders {
                                // Check if this folder has valid saved state
                                let folder_path_str = folder.path.to_string_lossy().to_string();

                                if let Some(ref setup) = this.pending_profile_load {
                                    if setup.validation.valid_folders.contains(&folder_path_str) {
                                        // Restore conversion status from saved state
                                        if let Some(saved) = setup.folder_states.get(&folder_path_str) {
                                            // Resolve output_dir path - for bundles it's relative
                                            let output_dir = if let Some(ref bundle_path) = setup.bundle_path {
                                                // Bundle format: resolve relative path
                                                bundle_path.join(&saved.output_dir)
                                            } else {
                                                // Legacy format: path is already absolute
                                                std::path::PathBuf::from(&saved.output_dir)
                                            };

                                            folder.conversion_status = FolderConversionStatus::Converted {
                                                output_dir,
                                                lossless_bitrate: saved.lossless_bitrate,
                                                output_size: saved.output_size,
                                                completed_at: 0,
                                            };
                                            println!("Restored conversion status for: {}", folder_path_str);
                                        }
                                    }
                                }

                                this.folders.push(folder);
                            }
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
                        for mut folder in folders {
                            // Check if this folder has valid saved state
                            let folder_path_str = folder.path.to_string_lossy().to_string();

                            if let Some(ref setup) = this.pending_profile_load {
                                if setup.validation.valid_folders.contains(&folder_path_str) {
                                    // Restore conversion status from saved state
                                    if let Some(saved) = setup.folder_states.get(&folder_path_str) {
                                        // Resolve output_dir path - for bundles it's relative
                                        let output_dir = if let Some(ref bundle_path) = setup.bundle_path {
                                            // Bundle format: resolve relative path
                                            bundle_path.join(&saved.output_dir)
                                        } else {
                                            // Legacy format: path is already absolute
                                            std::path::PathBuf::from(&saved.output_dir)
                                        };

                                        folder.conversion_status = FolderConversionStatus::Converted {
                                            output_dir,
                                            lossless_bitrate: saved.lossless_bitrate,
                                            output_size: saved.output_size,
                                            completed_at: 0,
                                        };
                                        println!("Restored conversion status for: {}", folder_path_str);
                                    }
                                }
                            }

                            this.folders.push(folder);
                        }
                    });
                }

                // Import complete - finalize profile loading
                let _ = this.update(&mut async_cx, |this, _cx| {
                    if let Some(setup) = this.pending_profile_load.take() {
                        println!("Profile import complete: {} folders loaded", this.folders.len());

                        // Restore ISO state if valid
                        if let Some(iso_path) = setup.iso_path {
                            if let Ok(iso_state) = crate::burning::IsoState::new(iso_path, &this.folders) {
                                this.iso_state = Some(iso_state);
                                println!("Restored ISO state from profile");
                            }
                        }

                        // Calculate bitrate and queue folders needing encoding
                        let folders_needing_encoding: Vec<_> = this.folders
                            .iter()
                            .filter(|f| !matches!(f.conversion_status, FolderConversionStatus::Converted { .. }))
                            .cloned()
                            .collect();

                        if !folders_needing_encoding.is_empty() {
                            let target_bitrate = this.calculated_bitrate();
                            println!("Profile loaded - calculated bitrate: {} kbps", target_bitrate);

                            // Update encoder with correct bitrate before queueing folders
                            if let Some(ref encoder) = this.background_encoder {
                                encoder.recalculate_bitrate(target_bitrate);
                            }

                            // Queue folders needing encoding
                            for folder in folders_needing_encoding {
                                this.queue_folder_for_encoding(&folder);
                            }

                            this.last_folder_change = Some(std::time::Instant::now());
                            this.last_calculated_bitrate = Some(target_bitrate);
                        }
                    }

                    // Notify encoder that import is complete (resumes encoding)
                    if let Some(ref encoder) = this.background_encoder {
                        encoder.import_complete();
                    }
                });

                let _ = async_cx.refresh();
            }
        }).detach();
    }
}

#[cfg(test)]
mod tests;
