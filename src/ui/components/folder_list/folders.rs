//! Folder operations for FolderList
//!
//! Handles folder addition, removal, reordering, and import polling.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui::{AsyncApp, Context, Timer, WeakEntity};

use crate::core::{find_album_folders, scan_music_folder, ImportState, MusicFolder};

use super::FolderList;

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

        // Clear manual bitrate override when adding folders (revert to auto-calculate)
        // Note: The actual bitrate recalculation and re-encoding happens after import
        // completes in start_import_polling(), which calls encoder.recalculate_bitrate()
        self.manual_bitrate_override = None;

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
                // Clear manual bitrate override (revert to auto-calculate)
                self.manual_bitrate_override = None;
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
            // Clear manual bitrate override (revert to auto-calculate)
            self.manual_bitrate_override = None;
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
                    let new_bitrate = this.calculated_bitrate();
                    if let Some(ref encoder) = this.background_encoder {
                        // Set the bitrate before resuming encoding
                        encoder.recalculate_bitrate(new_bitrate);
                        // Store the calculated bitrate
                        this.last_calculated_bitrate = Some(new_bitrate);
                        println!("Import complete - bitrate set to {} kbps", new_bitrate);
                    }
                    // Also check folders loaded from bundles that need re-encoding
                    // (they may have been encoded at a different bitrate)
                    this.queue_bundle_folders_for_reencoding(new_bitrate);
                });

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
}
