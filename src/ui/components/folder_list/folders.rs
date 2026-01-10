//! Folder operations for FolderList
//!
//! Handles folder addition, removal, reordering, and import polling.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui::{AsyncApp, Context, Timer, WeakEntity};

use crate::audio::is_audio_file;
use crate::core::{
    FolderId, ImportState, MusicFolder, find_album_folders, scan_audio_file, scan_music_folder,
};
use crate::ui::components::{TrackEditorUpdate, TrackEditorWindow, TrackEntry};

use super::{FolderList, PendingTrackEditorOpen};

impl FolderList {
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
    pub(super) fn contains_path(&self, path: &PathBuf) -> bool {
        self.folders.iter().any(|f| f.path == *path)
    }

    /// Add folders from external drop (Finder)
    ///
    /// Scans each folder asynchronously in a background thread.
    /// Only adds directories that aren't already in the list.
    pub fn add_external_folders(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        // Don't start if already importing
        if self.import_state.is_importing() {
            log::debug!("Import already in progress");
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

        // Clear manual bitrate override when adding folders (revert to auto-calculate)
        // Note: The actual bitrate recalculation and re-encoding happens after import
        // completes in start_import_polling(), which calls encoder.recalculate_bitrate()
        self.manual_bitrate_override = None;

        log::info!("Starting async import of {} folders", new_paths.len());

        // Reset import state (total will be updated after expansion)
        self.import_state.reset(new_paths.len());

        // Notify encoder that import is starting (delays encoding until complete)
        if let Some(ref encoder) = self.simple_encoder {
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

            log::debug!("Expanded to {} album folders", album_paths.len());

            // Reset state with actual count
            state.total.store(album_paths.len(), Ordering::SeqCst);

            for path in album_paths {
                log::debug!("Scanning: {}", path.display());
                match scan_music_folder(&path) {
                    Ok(folder) => {
                        log::debug!(
                            "Scanned folder: {} ({} files, {} bytes)",
                            folder.path.display(),
                            folder.file_count,
                            folder.total_size
                        );
                        state.push_folder(folder);
                    }
                    Err(e) => {
                        log::error!("Failed to scan folder {}: {}", path.display(), e);
                        // Still increment completed so we know when done
                        state.completed.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            state.finish();
            log::info!("Import complete");
        });

        // Start polling for results
        Self::start_import_polling(self.import_state.clone(), cx);
    }

    /// Add a single folder to the list
    #[allow(dead_code)]
    pub fn add_folder(&mut self, path: PathBuf) {
        if path.is_dir() && !self.contains_path(&path)
            && let Ok(folder) = scan_music_folder(&path) {
                // Queue for background encoding if available
                self.queue_folder_for_encoding(&folder);
                self.folders.push(folder);
                // Invalidate ISO since folder list changed
                self.iso_state = None;
                self.iso_generation_attempted = false;
                self.iso_has_been_burned = false;
                // Clear manual bitrate override (revert to auto-calculate)
                self.manual_bitrate_override = None;
                // Mark as having unsaved changes
                self.has_unsaved_changes = true;
                // Record change time for debounced bitrate recalculation
                self.last_folder_change = Some(std::time::Instant::now());
            }
    }

    /// Handle external drop from Finder
    ///
    /// Separates audio files from directories:
    /// - Directories are scanned as album folders
    /// - Audio files are combined into a new mixtape
    pub fn handle_external_drop(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        // Separate paths into directories and audio files
        let directories: Vec<PathBuf> = paths.iter()
            .filter(|p| p.is_dir())
            .cloned()
            .collect();

        let audio_files: Vec<PathBuf> = paths.iter()
            .filter(|p| p.is_file() && is_audio_file(p))
            .cloned()
            .collect();

        // Handle directories as album folders
        if !directories.is_empty() {
            self.add_external_folders(&directories, cx);
        }

        // Create mixtape if audio files were dropped
        if !audio_files.is_empty() {
            self.create_mixtape_from_files(&audio_files, cx);
        }
    }

    /// Create a new mixtape from dropped audio files
    fn create_mixtape_from_files(&mut self, paths: &[PathBuf], _cx: &mut Context<Self>) {
        // Scan each audio file
        let mut audio_files = Vec::new();
        for path in paths {
            match scan_audio_file(path) {
                Ok(info) => {
                    log::debug!("Scanned audio file: {:?}", path.file_name());
                    audio_files.push(info);
                }
                Err(e) => {
                    log::error!("Failed to scan audio file {}: {}", path.display(), e);
                }
            }
        }

        if audio_files.is_empty() {
            log::debug!("No valid audio files found");
            return;
        }

        // Create the mixtape folder
        let mixtape = MusicFolder::new_mixtape("My Mixtape".to_string(), audio_files);
        log::debug!(
            "Created mixtape with {} tracks, {} bytes",
            mixtape.file_count, mixtape.total_size
        );

        // Queue for encoding
        self.queue_folder_for_encoding(&mixtape);

        // Add to folder list
        self.folders.push(mixtape);

        // Invalidate ISO
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.iso_has_been_burned = false;

        // Clear manual bitrate override
        self.manual_bitrate_override = None;

        // Mark unsaved changes
        self.has_unsaved_changes = true;

        // Record change time
        self.last_folder_change = Some(std::time::Instant::now());

        // Open track editor for the new mixtape
        let mixtape_idx = self.folders.len() - 1;
        self.open_track_editor(mixtape_idx);
    }

    /// Add a new empty mixtape and open the track editor
    pub fn add_new_mixtape(&mut self, _cx: &mut Context<Self>) {
        let mixtape = MusicFolder::new_mixtape("My Mixtape".to_string(), Vec::new());
        log::debug!("Created new empty mixtape");

        // Add to folder list
        self.folders.push(mixtape);

        // Invalidate ISO
        self.iso_state = None;
        self.iso_generation_attempted = false;

        // Mark unsaved changes
        self.has_unsaved_changes = true;

        // Open track editor for the new mixtape
        let mixtape_idx = self.folders.len() - 1;
        self.open_track_editor(mixtape_idx);
    }

    /// Remove a folder by index
    pub fn remove_folder(&mut self, index: usize) {
        if index < self.folders.len() {
            let folder = self.folders.remove(index);

            // Notify encoder if available (removes from encoder's completed map)
            self.notify_folder_removed(&folder);
            // Invalidate ISO since folder list changed
            self.iso_state = None;
            self.iso_generation_attempted = false;
            self.iso_has_been_burned = false;
            // Clear manual bitrate override (revert to auto-calculate)
            self.manual_bitrate_override = None;
            // Clear cached bitrate to force fresh recalculation
            self.last_calculated_bitrate = None;
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
        // Clear manual bitrate override (revert to auto-calculate)
        self.manual_bitrate_override = None;
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

    /// Open the track editor for a folder at the given index
    ///
    /// This opens a new window for editing tracks in the folder.
    /// For albums: allows excluding tracks and reordering.
    /// For mixtapes: allows adding, removing, and reordering tracks.
    pub fn open_track_editor(&mut self, index: usize) {
        if index >= self.folders.len() {
            return;
        }

        // Check if already editing this folder
        if self.editing_folder_index == Some(index) {
            log::debug!("Track editor already open for folder {}", index);
            return;
        }

        let folder = &self.folders[index];
        log::debug!(
            "Opening track editor for folder {}: {} ({} tracks)",
            index,
            folder.path.display(),
            folder.audio_files.len()
        );

        // Set up the channel if not already created
        if self.track_editor_tx.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            self.track_editor_tx = Some(tx);
            self.track_editor_rx = Some(rx);
        }

        // Build track entries from folder data
        // For albums: share the folder's album art
        // For mixtapes: extract album art from each individual track
        let is_mixtape = folder.is_mixtape();
        let tracks: Vec<TrackEntry> = folder
            .audio_files
            .iter()
            .map(|f| {
                let track_meta = crate::audio::get_track_metadata(&f.path);
                TrackEntry {
                    file_info: f.clone(),
                    album_art: if is_mixtape {
                        crate::audio::get_album_art(&f.path)
                    } else {
                        folder.album_art.clone()
                    },
                    included: !folder.excluded_tracks.contains(&f.path),
                    title: track_meta.title,
                    artist: track_meta.artist,
                }
            })
            .collect();

        // Get display name
        let name = match &folder.kind {
            crate::core::FolderKind::Mixtape { name } => name.clone(),
            crate::core::FolderKind::Album => folder
                .album_name
                .clone()
                .unwrap_or_else(|| {
                    folder
                        .path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Unknown".to_string())
                }),
        };

        // Mark as editing
        self.editing_folder_index = Some(index);

        // Clone what we need before opening window (which requires &mut App)
        let folder_id = folder.id.clone();
        let folder_kind = folder.kind.clone();
        let existing_track_order = folder.track_order.clone();
        let tx = self.track_editor_tx.as_ref().unwrap().clone();

        // Store the data needed to open the window
        // We'll open it in the render loop since we need App context
        self.pending_track_editor_open = Some(PendingTrackEditorOpen {
            folder_id,
            folder_kind,
            name,
            tracks,
            update_tx: tx,
            existing_track_order,
        });
    }

    /// Handle updates from the track editor
    pub fn poll_track_editor_updates(&mut self) -> bool {
        // Collect updates first to avoid borrow conflicts
        let updates: Vec<TrackEditorUpdate> = match &self.track_editor_rx {
            Some(rx) => rx.try_iter().collect(),
            None => return false,
        };

        if updates.is_empty() {
            return false;
        }

        // Process collected updates
        for update in updates {
            match update {
                TrackEditorUpdate::OrderChanged { id, order } => {
                    self.handle_track_order_changed(&id, order);
                }
                TrackEditorUpdate::ExclusionsChanged { id, excluded } => {
                    self.handle_track_exclusions_changed(&id, excluded);
                }
                TrackEditorUpdate::TracksChanged {
                    id,
                    tracks,
                    album_arts,
                } => {
                    self.handle_mixtape_tracks_changed(&id, tracks, album_arts);
                }
                TrackEditorUpdate::NameChanged { id, name } => {
                    self.handle_mixtape_name_changed(&id, name);
                }
                TrackEditorUpdate::Closed { id } => {
                    self.handle_track_editor_closed(&id);
                }
            }
        }
        true
    }

    /// Handle track order change from editor
    ///
    /// Track reordering only requires regenerating the ISO staging (which applies
    /// numbered prefixes). No re-encoding is needed since output files are stable.
    fn handle_track_order_changed(&mut self, folder_id: &FolderId, order: Vec<usize>) {
        // Find index first to avoid borrow conflicts
        let idx = match self.folders.iter().position(|f| &f.id == folder_id) {
            Some(i) => i,
            None => return,
        };

        log::debug!("Track order changed for folder: {:?}", order);
        self.folders[idx].set_track_order(order);

        // Invalidate ISO so it gets regenerated with new track order
        // No need to re-encode - numbered prefixes are applied during ISO staging
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.has_unsaved_changes = true;
    }

    /// Handle track exclusion change from editor
    fn handle_track_exclusions_changed(&mut self, folder_id: &FolderId, excluded: Vec<PathBuf>) {
        // Find index first to avoid borrow conflicts
        let idx = match self.folders.iter().position(|f| &f.id == folder_id) {
            Some(i) => i,
            None => return,
        };

        log::debug!("Track exclusions changed: {} excluded", excluded.len());
        self.folders[idx].excluded_tracks = excluded;

        // Delete old output files (they include excluded tracks)
        if let Some(ref output_manager) = self.output_manager {
            let _ = output_manager.delete_folder_output_from_session(folder_id);
        }

        // Mark folder for re-encoding
        self.folders[idx].conversion_status = crate::core::FolderConversionStatus::NotConverted;
        // Invalidate ISO
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.has_unsaved_changes = true;
        // Re-queue for encoding (clone to avoid borrow conflict)
        let folder_clone = self.folders[idx].clone();
        self.queue_folder_for_encoding(&folder_clone);
    }

    /// Handle mixtape tracks change from editor
    fn handle_mixtape_tracks_changed(
        &mut self,
        folder_id: &FolderId,
        tracks: Vec<crate::core::AudioFileInfo>,
        _album_arts: Vec<Option<String>>,
    ) {
        // Find index first to avoid borrow conflicts
        let idx = match self.folders.iter().position(|f| &f.id == folder_id) {
            Some(i) => i,
            None => return,
        };

        log::debug!("Mixtape tracks changed: {} tracks", tracks.len());
        self.folders[idx].audio_files = tracks;
        self.folders[idx].recalculate_totals();
        // Mark folder for re-encoding
        self.folders[idx].conversion_status = crate::core::FolderConversionStatus::NotConverted;
        // Invalidate ISO
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.has_unsaved_changes = true;
        // Re-queue for encoding (clone to avoid borrow conflict)
        let folder_clone = self.folders[idx].clone();
        self.queue_folder_for_encoding(&folder_clone);
    }

    /// Handle mixtape name change from editor
    fn handle_mixtape_name_changed(&mut self, folder_id: &FolderId, name: String) {
        if let Some(folder) = self.folders.iter_mut().find(|f| &f.id == folder_id) {
            log::debug!("Mixtape name changed: {}", name);
            folder.set_mixtape_name(name);
            self.has_unsaved_changes = true;
        }
    }

    /// Handle track editor window closed
    fn handle_track_editor_closed(&mut self, folder_id: &FolderId) {
        log::debug!("Track editor closed for folder: {}", folder_id);
        // Always clear editing state - we only allow one editor at a time,
        // and the folder ID may have changed (e.g., after profile reload)
        self.editing_folder_index = None;
    }

    /// Open a pending track editor window (called from render loop)
    ///
    /// Uses cx.spawn to defer window opening outside the render cycle.
    pub fn open_pending_track_editor(&mut self, cx: &mut Context<Self>) {
        if let Some(pending) = self.pending_track_editor_open.take() {
            // Spawn a task to open the window outside the render cycle
            cx.spawn(|_this, cx: &mut AsyncApp| {
                let async_cx = cx.clone();
                async move {
                    async_cx
                        .update(|cx| {
                            let _window_handle = TrackEditorWindow::open(
                                cx,
                                pending.folder_id,
                                pending.folder_kind,
                                pending.name,
                                pending.tracks,
                                pending.update_tx,
                                pending.existing_track_order,
                            );
                        })
                        .ok();
                }
            })
            .detach();
        }
    }

    /// Start a polling loop that drains imported folders and updates the UI
    pub(super) fn start_import_polling(state: ImportState, cx: &mut Context<Self>) {
        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                loop {
                    let cx_for_after_await = async_cx.clone();

                    // Wait 50ms between updates
                    Timer::after(Duration::from_millis(50)).await;

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
                            // NOTE: Don't set last_folder_change during import - we'll calculate
                            // bitrate once after all folders are imported to avoid mid-import recalcs
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
                    });
                }

                // Calculate and set bitrate BEFORE resuming encoding
                // This ensures all folders are accounted for in the bitrate calculation
                let _ = this.update(&mut async_cx, |this, _cx| {
                    // Clear cached bitrate to force fresh calculation with all folders
                    this.last_calculated_bitrate = None;
                    let new_bitrate = this.calculated_bitrate();
                    if let Some(ref encoder) = this.simple_encoder {
                        // Set the bitrate before resuming encoding
                        encoder.recalculate_bitrate(new_bitrate);
                        // Store the calculated bitrate
                        this.last_calculated_bitrate = Some(new_bitrate);
                        log::info!("Import complete - bitrate set to {} kbps", new_bitrate);
                    }
                });

                // Notify encoder that import is complete (resumes encoding)
                let _ = this.update(&mut async_cx, |this, _cx| {
                    if let Some(ref encoder) = this.simple_encoder {
                        encoder.import_complete();
                    }
                });

                let _ = async_cx.refresh();
            }
        })
        .detach();
    }
}
