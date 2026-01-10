//! Profile management for FolderList
//!
//! Handles save/load profiles, volume label dialog, and profile import polling.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, PathPromptOptions, PromptLevel, Timer, WeakEntity, Window,
};

use crate::actions::take_pending_files;
use crate::core::{FolderConversionStatus, FolderKind, ImportState, scan_music_folder};
use crate::profiles::types::SavedFolderKind;

use super::{FolderList, PendingBurnAction, VolumeLabelDialog};

/// Default CD volume label
const DEFAULT_LABEL: &str = "Untitled MP3CD";

impl FolderList {
    /// Poll for volume label updates from the dialog
    ///
    /// Returns true if a label was received (useful for knowing if UI needs refresh)
    pub(super) fn poll_volume_label(&mut self) -> bool {
        if let Some(ref rx) = self.pending_volume_label_rx
            && let Ok(label) = rx.try_recv() {
                log::debug!("Volume label set to: {}", label);

                // If the label changed, invalidate the ISO so it gets regenerated
                if self.volume_label != label
                    && self.iso_state.is_some() {
                        log::debug!("Volume label changed - invalidating existing ISO");
                        self.iso_state = None;
                        self.iso_generation_attempted = false;
                    }

                self.volume_label = label;
                // Clear the receiver since we got the label
                self.pending_volume_label_rx = None;
                // Note: pending_burn_action will be handled in check_pending_burn_action
                return true;
            }
        false
    }

    /// Check for files opened via Finder and load them
    ///
    /// This should be called from the render loop. When a user double-clicks
    /// a .mp3cd file in Finder, macOS opens our app with that file. The path
    /// is stored in a static and we poll for it here.
    pub(super) fn poll_pending_open_files(&mut self, cx: &mut Context<Self>) {
        let pending_paths = take_pending_files();
        for path in pending_paths {
            log::debug!("Loading profile from Finder: {:?}", path);
            // Don't prompt to save - just load the profile directly
            // (this is the expected behavior when double-clicking a file)
            if let Err(e) = self.load_profile(&path, cx) {
                log::error!("Failed to load profile: {}", e);
            }
        }
    }

    /// Save the current state as a profile to the specified path
    ///
    /// If `for_bundle` is true, saves as v2.0 bundle format including converted files.
    /// If `for_bundle` is false, saves as legacy format (metadata only).
    pub fn save_profile(
        &mut self,
        path: &std::path::Path,
        profile_name: String,
        for_bundle: bool,
    ) -> Result<(), String> {
        // Save volume_label if it's not the default
        let volume_label = if self.volume_label == DEFAULT_LABEL {
            None
        } else {
            Some(self.volume_label.clone())
        };

        // If saving as bundle and we have converted files in temp, copy them to bundle first
        if for_bundle
            && let Some(output_manager) = &self.output_manager {
                // Collect folder IDs that have been converted
                let converted_folder_ids: Vec<crate::core::FolderId> = self
                    .folders
                    .iter()
                    .filter_map(|f| {
                        if matches!(
                            f.conversion_status,
                            FolderConversionStatus::Converted { .. }
                        ) {
                            Some(f.id.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                if !converted_folder_ids.is_empty() && !output_manager.is_bundle_mode() {
                    // If path exists as a file (old metadata-only profile), remove it first
                    // so we can create a bundle directory in its place
                    if path.is_file() {
                        std::fs::remove_file(path)
                            .map_err(|e| format!("Failed to remove existing profile file: {}", e))?;
                        log::debug!("Removed existing metadata-only profile to create bundle");
                    }

                    // Copy converted files from temp to bundle
                    output_manager.copy_to_bundle(path, &converted_folder_ids)?;
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
            self.manual_bitrate_override,
            for_bundle,
        )?;

        // If we saved as bundle, update output manager to use the bundle path for future encodes
        if for_bundle
            && let Some(output_manager) = &mut self.output_manager {
                output_manager.set_bundle_path(Some(path.to_path_buf()));
            }

        Ok(())
    }

    /// Check if any folders have been converted
    pub(super) fn has_converted_folders(&self) -> bool {
        self.folders.iter().any(|f| {
            matches!(
                f.conversion_status,
                FolderConversionStatus::Converted { .. }
            )
        })
    }

    /// Check if any folders have missing source files
    ///
    /// Returns true if any folder was loaded from a bundle but the original
    /// source files are not accessible. These folders cannot be re-encoded.
    pub fn has_source_unavailable_folders(&self) -> bool {
        self.folders.iter().any(|f| !f.source_available)
    }

    /// Get count of folders with unavailable source
    pub fn source_unavailable_count(&self) -> usize {
        self.folders.iter().filter(|f| !f.source_available).count()
    }

    /// Load a profile and restore its state
    ///
    /// This will:
    /// 1. Load the profile metadata from disk (fast)
    /// 2. Validate the saved conversion state (fast)
    /// 3. Scan folders asynchronously (background thread)
    /// 4. Restore conversion status for valid folders
    /// 5. Queue folders needing re-encoding to the background encoder
    pub fn load_profile(
        &mut self,
        path: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
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

        log::debug!("Starting async profile load of {} folders", folder_count);

        // Clear current state
        self.folders.clear();
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.iso_has_been_burned = false;
        self.last_calculated_bitrate = None; // Force fresh calculation on load

        // Remember the profile path and mark as saved (no unsaved changes after load)
        self.current_profile_path = Some(path.to_path_buf());
        self.has_unsaved_changes = false;

        // Restore volume label from profile (or default if not saved)
        self.volume_label = setup
            .volume_label
            .clone()
            .unwrap_or_else(|| DEFAULT_LABEL.to_string());

        // Restore manual bitrate override from profile (or reset to auto-calculate)
        self.manual_bitrate_override = setup.manual_bitrate_override;

        // DON'T set bundle_path when loading - new encodes should always go to temp.
        // The bundle is a read-only snapshot until the user explicitly saves.
        // Bundle files will be copied to temp during import, so we always clean first.
        if let Some(output_manager) = &mut self.output_manager {
            output_manager.set_bundle_path(None);
        }

        // Clear the encoder state and delete converted files
        // Always clear temp, even for bundles - bundle files get copied to temp anyway
        if let Some(encoder) = &self.simple_encoder {
            encoder.clear_all();
        }

        // Store the setup for the polling callback
        self.pending_profile_load = Some(setup.clone());

        // Reset import state
        self.import_state.reset(folder_count);

        // Notify encoder that import is starting (delays encoding until complete)
        if let Some(ref encoder) = self.simple_encoder {
            encoder.import_started();
        }

        // Clone state for background thread
        let state = self.import_state.clone();
        let folder_paths = setup.folder_paths.clone();
        let folder_states = setup.folder_states.clone();
        let bundle_path = setup.bundle_path.clone();

        // Spawn background thread for scanning
        std::thread::spawn(move || {
            for path in folder_paths {
                let path_str = path.to_string_lossy().to_string();

                // Check if this is a mixtape (empty path with Mixtape kind in saved state)
                if path_str.is_empty() {
                    if let Some(saved) = folder_states.get(&path_str) {
                        if let SavedFolderKind::Mixtape { name, tracks } = &saved.kind {
                            log::debug!("Restoring mixtape: {}", name);
                            // Convert saved tracks to SavedMixtapeTrackInfo
                            let track_infos: Vec<crate::core::SavedMixtapeTrackInfo> = tracks
                                .iter()
                                .map(|t| crate::core::SavedMixtapeTrackInfo {
                                    source_path: t.source_path.clone(),
                                    duration: t.duration,
                                    bitrate: t.bitrate,
                                    size: t.size,
                                    codec: t.codec.clone(),
                                    is_lossy: t.is_lossy,
                                })
                                .collect();

                            let folder = crate::core::create_mixtape_from_saved_state(
                                saved.folder_id.clone(),
                                name.clone(),
                                track_infos,
                                saved.album_art.clone(),
                            );
                            log::debug!(
                                "Restored mixtape: {} ({} tracks)",
                                name,
                                folder.file_count
                            );
                            state.push_folder(folder);
                            continue;
                        }
                    }
                    // Empty path but not a mixtape - skip it
                    log::warn!("Skipping empty path that is not a mixtape");
                    continue;
                }

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

                        // For bundle profiles with saved metadata, create folder from metadata
                        if bundle_path.is_some()
                            && let Some(saved) = folder_states.get(&path_str)
                            && saved.has_display_metadata()
                        {
                            log::debug!(
                                "Creating folder from saved metadata (source unavailable): {}",
                                path.display()
                            );

                            // Extract folder kind from saved state
                            let (kind, excluded_tracks, track_order) = match &saved.kind {
                                SavedFolderKind::Album {
                                    excluded_tracks,
                                    track_order,
                                } => (
                                    Some(FolderKind::Album),
                                    Some(
                                        excluded_tracks
                                            .iter()
                                            .map(|p| PathBuf::from(p))
                                            .collect(),
                                    ),
                                    track_order.clone(),
                                ),
                                SavedFolderKind::Mixtape { name, tracks } => {
                                    // For mixtapes in bundle with missing source, reconstruct with tracks
                                    log::debug!("Restoring mixtape from bundle: {}", name);
                                    let track_infos: Vec<crate::core::SavedMixtapeTrackInfo> = tracks
                                        .iter()
                                        .map(|t| crate::core::SavedMixtapeTrackInfo {
                                            source_path: t.source_path.clone(),
                                            duration: t.duration,
                                            bitrate: t.bitrate,
                                            size: t.size,
                                            codec: t.codec.clone(),
                                            is_lossy: t.is_lossy,
                                        })
                                        .collect();

                                    let folder = crate::core::create_mixtape_from_saved_state(
                                        saved.folder_id.clone(),
                                        name.clone(),
                                        track_infos,
                                        saved.album_art.clone(),
                                    );
                                    state.push_folder(folder);
                                    continue;
                                }
                            };

                            // Create a MusicFolder from the saved metadata
                            let folder = crate::core::create_folder_from_metadata(
                                saved.folder_id.clone(),
                                path.clone(),
                                saved.file_count as u32,
                                saved.source_size.unwrap_or(0),
                                saved.total_duration.unwrap_or(0.0),
                                saved.album_name.clone(),
                                saved.artist_name.clone(),
                                saved.year.clone(),
                                saved.album_art.clone(),
                                // Will be updated later during conversion status restoration
                                crate::core::FolderConversionStatus::NotConverted,
                                kind,
                                excluded_tracks,
                                track_order,
                            );
                            state.push_folder(folder);
                        } else {
                            // Can't recover - record as failed for error reporting
                            state.push_failed(path.clone());
                        }
                    }
                }
            }
            state.finish();
            log::debug!("Profile import complete");
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
                            log::debug!("User chose to save - showing save dialog");
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
                            log::debug!("User chose not to save - clearing");
                            let _ = async_cx.update(|cx| {
                                let _ = this_handle.update(cx, |this, cx| {
                                    this.clear_for_new_profile(cx);
                                });
                            });
                        }
                        2 => {
                            // Cancel - do nothing
                            log::debug!("User cancelled new profile");
                        }
                        _ => {}
                    }
                }
            }
        })
        .detach();
    }

    /// Actually clear the state for a new profile
    pub(super) fn clear_for_new_profile(&mut self, cx: &mut Context<Self>) {
        self.folders.clear();
        self.iso_state = None;
        self.iso_generation_attempted = false;
        self.iso_has_been_burned = false;
        self.last_folder_change = None;
        self.last_calculated_bitrate = None;
        self.manual_bitrate_override = None; // Reset to auto-calculate
        self.volume_label = DEFAULT_LABEL.to_string();
        self.current_profile_path = None;
        self.has_unsaved_changes = false;
        // Clear the encoder state and delete converted files
        if let Some(encoder) = &self.simple_encoder {
            encoder.clear_all();
        }
        // Clear bundle path so new encodes go to temp directory, not the old bundle
        if let Some(output_manager) = &mut self.output_manager {
            output_manager.set_bundle_path(None);
        }
        log::debug!("New profile - cleared all folders and encoder state");
        cx.notify();
    }

    /// Show the volume label dialog
    ///
    /// Opens a modal dialog for editing the CD volume label.
    /// If `pending_action` is provided, that action will be triggered after the dialog closes.
    pub(super) fn show_volume_label_dialog(
        &mut self,
        pending_action: Option<PendingBurnAction>,
        cx: &mut Context<Self>,
    ) {
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
                            log::debug!("User chose to save before opening - showing save dialog");
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
                            log::debug!("User chose not to save - opening profile");
                            let _ = async_cx.update(|cx| {
                                let _ = this_handle.update(cx, |this, cx| {
                                    this.show_open_file_picker(cx);
                                });
                            });
                        }
                        2 => {
                            // Cancel - do nothing
                            log::debug!("User cancelled open profile");
                        }
                        _ => {}
                    }
                }
            }
        })
        .detach();
    }

    /// Load a dropped profile file, with unsaved changes check
    ///
    /// Similar to open_profile but for a specific path (from drag-drop).
    pub fn load_dropped_profile(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If no unsaved changes, load immediately
        if !self.has_unsaved_changes {
            if let Err(e) = self.load_profile(&path, cx) {
                log::error!("Failed to load dropped profile: {}", e);
            }
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
                            // Save first, then load dropped profile
                            log::debug!("User chose to save before loading dropped profile");
                            let _ = async_cx.update_window(window_handle, |_, window, cx| {
                                let _ = this_handle.update(cx, |this, cx| {
                                    // Store the path to load after save completes
                                    this.pending_load_profile_path = Some(path);
                                    this.save_profile_dialog(window, cx);
                                });
                            });
                        }
                        1 => {
                            // Don't Save - load profile directly
                            log::debug!("User chose not to save - loading dropped profile");
                            let _ = async_cx.update(|cx| {
                                let _ = this_handle.update(cx, |this, cx| {
                                    if let Err(e) = this.load_profile(&path, cx) {
                                        log::error!("Failed to load dropped profile: {}", e);
                                    }
                                });
                            });
                        }
                        2 => {
                            // Cancel - do nothing
                            log::debug!("User cancelled loading dropped profile");
                        }
                        _ => {}
                    }
                }
            }
        })
        .detach();
    }

    /// Actually show the file picker to open a profile
    pub(super) fn show_open_file_picker(&mut self, cx: &mut Context<Self>) {
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
                if let Ok(Ok(Some(paths))) = receiver.await
                    && let Some(path) = paths.first() {
                        let path = path.clone();
                        let _ = this_handle.update(&mut async_cx, |this, cx| {
                            if let Err(e) = this.load_profile(&path, cx) {
                                log::error!("Failed to load profile: {}", e);
                            }
                        });
                    }
            }
        })
        .detach();
    }

    /// Show save dialog to save current profile (called from File > Save menu)
    ///
    /// If there are converted folders, shows a dialog asking whether to include audio files.
    /// Then shows the file picker and saves the profile.
    pub fn save_profile_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.folders.is_empty() {
            log::debug!("No folders to save");
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
                                        this.pending_load_profile_path = None;
                                    });
                                });
                            }
                        }
                    }
                }
            })
            .detach();
        } else {
            // No converted folders - just show file picker (save as metadata only)
            self.show_save_file_picker(window, cx, false);
        }
    }

    /// Internal method to show the save file picker
    pub(super) fn show_save_file_picker(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
        for_bundle: bool,
    ) {
        // Use current profile path if we've saved before, otherwise generate from first folder
        let (start_dir, default_filename) =
            if let Some(ref current_path) = self.current_profile_path {
                // Use the directory and filename from the current profile
                let dir = current_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| dirs::document_dir().unwrap_or_else(|| PathBuf::from(".")));
                // Get just the file stem (name without any extension) and add .mp3cd
                let stem = current_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Untitled".to_string());
                let filename = format!("{}.mp3cd", stem);
                (dir, filename)
            } else {
                // Generate a default filename from the first folder
                let default_name = self
                    .folders
                    .first()
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
                        let chosen_name = path
                            .file_stem()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| profile_name);

                        let _ = this_handle.update(&mut async_cx, |this, cx| {
                            if let Err(e) = this.save_profile(&path, chosen_name, for_bundle) {
                                log::error!("Failed to save profile: {}", e);
                                this.pending_new_after_save = false;
                                this.pending_open_after_save = false;
                                this.pending_load_profile_path = None;
                            } else {
                                log::debug!("Profile saved to: {:?} (bundle: {})", path, for_bundle);
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
                                // If we were saving before loading a dropped profile, load it now
                                if let Some(profile_path) = this.pending_load_profile_path.take()
                                    && let Err(e) = this.load_profile(&profile_path, cx) {
                                        log::error!("Failed to load dropped profile: {}", e);
                                    }
                            }
                        });
                    }
                    _ => {
                        // Cancelled or error - reset the flags
                        let _ = this_handle.update(&mut async_cx, |this, _cx| {
                            this.pending_new_after_save = false;
                            this.pending_open_after_save = false;
                            this.pending_load_profile_path = None;
                        });
                    }
                }
            }
        })
        .detach();
    }

    /// Start a polling loop for profile import that restores conversion status
    ///
    /// This is similar to `start_import_polling` but handles restoration of
    /// conversion status from the saved profile state.
    pub(super) fn start_profile_import_polling(state: ImportState, cx: &mut Context<Self>) {
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
                            for mut folder in folders {
                                // Check if this folder has valid saved state
                                let folder_path_str = folder.path.to_string_lossy().to_string();

                                // Restore conversion status only if:
                                // 1. Source is available and folder is valid (not modified)
                                // 2. Source is unavailable (created from metadata) - restore anyway
                                let should_restore_conversion = if let Some(ref setup) = this.pending_profile_load {
                                    setup.validation.valid_folders.contains(&folder_path_str)
                                        || !folder.source_available
                                } else {
                                    false
                                };

                                // Always restore folder preferences (exclusions, track order)
                                // regardless of whether conversion status can be restored
                                if let Some(ref setup) = this.pending_profile_load
                                    && let Some(saved) = setup.folder_states.get(&folder_path_str)
                                {
                                    // Restore folder kind (exclusions and track order)
                                    // These are user preferences and should always be restored
                                    match &saved.kind {
                                        SavedFolderKind::Album {
                                            excluded_tracks,
                                            track_order,
                                        } => {
                                            folder.excluded_tracks = excluded_tracks
                                                .iter()
                                                .map(|p| PathBuf::from(p))
                                                .collect();
                                            folder.track_order = track_order.clone();
                                            if track_order.is_some() {
                                                log::debug!(
                                                    "Restored track order for: {}",
                                                    folder_path_str
                                                );
                                            }
                                        }
                                        SavedFolderKind::Mixtape { name, .. } => {
                                            folder.kind =
                                                FolderKind::Mixtape { name: name.clone() };
                                        }
                                    }

                                    // Restore conversion status if valid
                                    if should_restore_conversion {
                                        // Resolve output_dir path - for bundles it's relative
                                        let output_dir = if let Some(ref bundle_path) = setup.bundle_path {
                                            // Bundle format: resolve relative path
                                            bundle_path.join(&saved.output_dir)
                                        } else {
                                            // Legacy format: path is already absolute
                                            std::path::PathBuf::from(&saved.output_dir)
                                        };

                                        // Measure actual output size instead of using saved value
                                        // This ensures the UI shows correct sizes even if files
                                        // were re-encoded since the profile was saved
                                        let actual_output_size = if output_dir.exists() {
                                            crate::conversion::calculate_dir_size(&output_dir)
                                                .unwrap_or(saved.output_size)
                                        } else {
                                            saved.output_size // Fallback if dir doesn't exist
                                        };

                                        folder.conversion_status = FolderConversionStatus::Converted {
                                            output_dir,
                                            lossless_bitrate: saved.lossless_bitrate,
                                            output_size: actual_output_size,
                                            completed_at: saved.completed_at.unwrap_or(0),
                                        };

                                        if folder.source_available {
                                            log::debug!(
                                                "Restored conversion status for: {}",
                                                folder_path_str
                                            );
                                        } else {
                                            log::debug!(
                                                "Restored conversion status (source unavailable): {}",
                                                folder_path_str
                                            );
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

                            // Restore conversion status if:
                            // 1. Source is available and folder is valid (not modified)
                            // 2. Source is unavailable (created from metadata) - restore anyway
                            let should_restore = if let Some(ref setup) = this.pending_profile_load {
                                setup.validation.valid_folders.contains(&folder_path_str)
                                    || !folder.source_available
                            } else {
                                false
                            };

                            if should_restore
                                && let Some(ref setup) = this.pending_profile_load
                                && let Some(saved) = setup.folder_states.get(&folder_path_str)
                            {
                                // Resolve output_dir path - for bundles it's relative
                                let output_dir = if let Some(ref bundle_path) = setup.bundle_path {
                                    // Bundle format: resolve relative path
                                    bundle_path.join(&saved.output_dir)
                                } else {
                                    // Legacy format: path is already absolute
                                    std::path::PathBuf::from(&saved.output_dir)
                                };

                                // Measure actual output size instead of using saved value
                                // This ensures the UI shows correct sizes even if files
                                // were re-encoded since the profile was saved
                                let actual_output_size = if output_dir.exists() {
                                    crate::conversion::calculate_dir_size(&output_dir)
                                        .unwrap_or(saved.output_size)
                                } else {
                                    saved.output_size // Fallback if dir doesn't exist
                                };

                                folder.conversion_status = FolderConversionStatus::Converted {
                                    output_dir,
                                    lossless_bitrate: saved.lossless_bitrate,
                                    output_size: actual_output_size,
                                    completed_at: saved.completed_at.unwrap_or(0),
                                };

                                if folder.source_available {
                                    log::debug!(
                                        "Restored conversion status for: {}",
                                        folder_path_str
                                    );
                                } else {
                                    log::debug!(
                                        "Restored conversion status (source unavailable): {}",
                                        folder_path_str
                                    );
                                }
                            }

                            this.folders.push(folder);
                        }
                    });
                }

                // Import complete - finalize profile loading
                let failed_paths = state.get_failed_paths();
                let _ = this.update(&mut async_cx, |this, _cx| {
                    if let Some(setup) = this.pending_profile_load.take() {
                        log::debug!(
                            "Profile import complete: {} folders loaded",
                            this.folders.len()
                        );

                        // Check for failed folders and prepare error message
                        if !failed_paths.is_empty() {
                            // Extract unique volume/path roots
                            let unique_roots: std::collections::HashSet<String> = failed_paths
                                .iter()
                                .filter_map(|p| {
                                    // Get first two path components (e.g., /Volumes/MediaDrive)
                                    let components: Vec<_> = p.components().take(3).collect();
                                    if components.len() >= 3 {
                                        Some(
                                            components
                                                .iter()
                                                .map(|c| c.as_os_str().to_string_lossy())
                                                .collect::<Vec<_>>()
                                                .join("/"),
                                        )
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            let message = if unique_roots.len() == 1 {
                                let root = unique_roots.iter().next().unwrap();
                                format!(
                                    "{} folder{} could not be loaded because the source files are not accessible.\n\n\
                                    Please connect or mount: {}\n\n\
                                    Then reload the profile to access these folders.",
                                    failed_paths.len(),
                                    if failed_paths.len() == 1 { "" } else { "s" },
                                    root
                                )
                            } else {
                                let roots: Vec<_> = unique_roots.into_iter().collect();
                                format!(
                                    "{} folder{} could not be loaded because the source files are not accessible.\n\n\
                                    Please connect or mount these locations:\n• {}\n\n\
                                    Then reload the profile to access these folders.",
                                    failed_paths.len(),
                                    if failed_paths.len() == 1 { "" } else { "s" },
                                    roots.join("\n• ")
                                )
                            };

                            this.pending_error_message = Some((
                                "Could Not Load All Folders".to_string(),
                                message,
                            ));
                        }

                        // Restore ISO state if valid
                        if let Some(iso_path) = setup.iso_path
                            && let Ok(iso_state) =
                                crate::burning::IsoState::new(iso_path, &this.folders)
                            {
                                this.iso_state = Some(iso_state);
                                log::debug!("Restored ISO state from profile");
                            }

                        // For bundle profiles: copy converted files from bundle to temp
                        // and register with encoder. This unifies bundle folders with
                        // encoder-tracked folders - no more bifurcation!
                        if let Some(ref bundle_path) = setup.bundle_path {
                            for folder in &mut this.folders {
                                if let FolderConversionStatus::Converted {
                                    output_size,
                                    lossless_bitrate,
                                    completed_at,
                                    ..
                                } = &folder.conversion_status
                                {
                                    // Copy from bundle to temp
                                    if let Some(ref output_manager) = this.output_manager {
                                        match output_manager
                                            .copy_from_bundle(bundle_path, &folder.id)
                                        {
                                            Ok(new_output_dir) => {
                                                log::debug!(
                                                    "Copied bundle folder to temp: {}",
                                                    folder.path.display()
                                                );

                                                // Register with encoder as already completed
                                                if let Some(ref encoder) = this.simple_encoder {
                                                    encoder.register_completed(
                                                        folder.clone(),
                                                        new_output_dir.clone(),
                                                        *output_size,
                                                        *lossless_bitrate,
                                                        *completed_at,
                                                    );
                                                }

                                                // Update folder's conversion status to point to temp
                                                folder.conversion_status =
                                                    FolderConversionStatus::Converted {
                                                        output_dir: new_output_dir,
                                                        output_size: *output_size,
                                                        lossless_bitrate: *lossless_bitrate,
                                                        completed_at: *completed_at,
                                                    };
                                            }
                                            Err(e) => {
                                                log::error!(
                                                    "Failed to copy bundle folder {}: {}",
                                                    folder.path.display(),
                                                    e
                                                );
                                                // Only reset to NotConverted if source is available
                                                // for re-encoding. Otherwise this folder is lost.
                                                if folder.source_available {
                                                    folder.conversion_status =
                                                        FolderConversionStatus::NotConverted;
                                                } else {
                                                    log::error!(
                                                        "WARNING: Folder {} has no source files and bundle copy failed - folder will be removed",
                                                        folder.path.display()
                                                    );
                                                    // Keep as NotConverted - it will be filtered out
                                                    // since it can't be encoded or burned
                                                    folder.conversion_status =
                                                        FolderConversionStatus::NotConverted;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            // Legacy (non-bundle) profile: register converted folders with encoder
                            // so they're tracked in the unified system
                            for folder in &this.folders {
                                if let FolderConversionStatus::Converted {
                                    output_dir,
                                    output_size,
                                    lossless_bitrate,
                                    completed_at,
                                } = &folder.conversion_status
                                {
                                    if let Some(ref encoder) = this.simple_encoder {
                                        encoder.register_completed(
                                            folder.clone(),
                                            output_dir.clone(),
                                            *output_size,
                                            *lossless_bitrate,
                                            *completed_at,
                                        );
                                    }
                                }
                            }
                        }

                        // Queue folders needing encoding (those that aren't Converted AND have source)
                        let folders_needing_encoding: Vec<_> = this
                            .folders
                            .iter()
                            .filter(|f| {
                                !matches!(
                                    f.conversion_status,
                                    FolderConversionStatus::Converted { .. }
                                ) && f.source_available
                            })
                            .cloned()
                            .collect();

                        // Remove folders that can't be encoded or burned
                        // (no source and no converted files)
                        let folders_to_remove: Vec<_> = this
                            .folders
                            .iter()
                            .filter(|f| {
                                !f.source_available
                                    && !matches!(
                                        f.conversion_status,
                                        FolderConversionStatus::Converted { .. }
                                    )
                            })
                            .map(|f| f.path.clone())
                            .collect();

                        if !folders_to_remove.is_empty() {
                            log::debug!(
                                "Removing {} folders with no source and no converted files",
                                folders_to_remove.len()
                            );
                            this.folders
                                .retain(|f| !folders_to_remove.contains(&f.path));
                        }

                        if !folders_needing_encoding.is_empty() {
                            // Don't lock in a preliminary bitrate estimate - let the encoder
                            // calculate the actual optimal bitrate after measuring lossy sizes.
                            // The preliminary estimate uses conservative margins that may be too tight.
                            if let Some(ref encoder) = this.simple_encoder {
                                // Pass 0 to set auto-calculate mode (no manual override)
                                encoder.recalculate_bitrate(0);
                            }

                            // Queue folders needing encoding
                            for folder in folders_needing_encoding {
                                this.queue_folder_for_encoding(&folder);
                            }

                            // DON'T set last_folder_change here - we want the encoder to
                            // auto-calculate bitrate after measuring actual lossy sizes.
                            // Setting last_folder_change would trigger check_debounced_bitrate_recalculation()
                            // which would override with the preliminary estimate.
                        } else {
                            // All folders already encoded - restore lossless_bitrate from saved state
                            // Find any folder with a lossless_bitrate and use that
                            let saved_lossless_bitrate = this.folders.iter().find_map(|f| {
                                if let FolderConversionStatus::Converted {
                                    lossless_bitrate, ..
                                } = &f.conversion_status
                                {
                                    *lossless_bitrate
                                } else {
                                    None
                                }
                            });
                            if let Some(bitrate) = saved_lossless_bitrate {
                                log::debug!(
                                    "Profile loaded - restored lossless bitrate: {} kbps",
                                    bitrate
                                );
                                this.last_calculated_bitrate = Some(bitrate);
                            }
                        }
                    }

                    // Check if we have folders loaded from bundle without source
                    let source_unavailable_count = this
                        .folders
                        .iter()
                        .filter(|f| {
                            !f.source_available
                                && matches!(
                                    f.conversion_status,
                                    FolderConversionStatus::Converted { .. }
                                )
                        })
                        .count();

                    if source_unavailable_count > 0 {
                        let folder_word = if source_unavailable_count == 1 {
                            "folder"
                        } else {
                            "folders"
                        };
                        this.pending_info_message = Some((
                            "Source Files Not Available".to_string(),
                            format!(
                                "{} {} loaded from saved converted files.\n\n\
                                You can still burn a CD, but cannot re-encode at a different bitrate \
                                without reconnecting the source volume.",
                                source_unavailable_count,
                                folder_word
                            ),
                        ));
                    }

                    // Sync manual bitrate override to encoder (if set in profile)
                    if let Some(ref encoder) = this.simple_encoder {
                        if let Some(override_bitrate) = this.manual_bitrate_override {
                            encoder.recalculate_bitrate(override_bitrate);
                        }
                        // Notify encoder that import is complete (resumes encoding)
                        encoder.import_complete();
                    }
                });

                let _ = async_cx.refresh();
            }
        })
        .detach();
    }
}
