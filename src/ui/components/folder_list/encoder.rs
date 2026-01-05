//! Background encoder management for FolderList
//!
//! Handles background encoding, event polling, and encoder state queries.

use std::time::Duration;

use gpui::{AsyncApp, Context, Timer, WeakEntity};

use crate::conversion::{
    BackgroundEncoder, BackgroundEncoderHandle, EncoderEvent, EncodingPhase, OutputManager,
};
use crate::core::{FolderConversionStatus, FolderId};

use super::FolderList;

impl FolderList {
    /// Initialize the background encoder for immediate folder conversion
    ///
    /// This should be called after construction when background encoding is desired.
    /// If not called, folders will only be converted when "Burn" is clicked (legacy mode).
    /// Returns a clone of the encoder handle so it can be stored as a global.
    pub fn enable_background_encoding(&mut self) -> Result<BackgroundEncoderHandle, String> {
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

        println!(
            "Background encoding enabled, session: {:?}",
            self.output_manager.as_ref().map(|m| m.session_id())
        );

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

    /// Get the current encoding phase (for UI display logic)
    #[allow(dead_code)]
    pub fn get_encoding_phase(&self) -> EncodingPhase {
        if let Some(ref encoder) = self.background_encoder {
            let state = encoder.get_state();
            let guard = state.lock().unwrap();
            guard.encoding_phase
        } else {
            EncodingPhase::Idle
        }
    }

    /// Check if the bitrate is preliminary (will be recalculated after lossy encoding)
    ///
    /// Returns true when:
    /// - We're in LossyPass (actively encoding lossy files), OR
    /// - We're in Idle with lossless files pending (before encoding starts)
    ///
    /// Returns false during LosslessPass (we have the final optimized bitrate).
    pub fn is_bitrate_preliminary(&self) -> bool {
        if let Some(ref encoder) = self.background_encoder {
            let state = encoder.get_state();
            let guard = state.lock().unwrap();
            match guard.encoding_phase {
                // During LossyPass, bitrate is always preliminary
                EncodingPhase::LossyPass => true,
                // During Idle, preliminary only if lossless files are pending
                EncodingPhase::Idle => !guard.lossless_pending.is_empty(),
                // During LosslessPass, we have the final bitrate
                EncodingPhase::LosslessPass => false,
            }
        } else {
            false
        }
    }

    /// Update encoder's embed_album_art setting
    #[allow(dead_code)]
    pub fn set_embed_album_art(&self, embed: bool) {
        if let Some(ref encoder) = self.background_encoder {
            println!("[FolderList] Sending embed_album_art={} to encoder", embed);
            encoder.set_embed_album_art(embed);
        } else {
            println!("[FolderList] WARNING: No encoder to send embed_album_art to!");
        }
    }

    /// Queue a folder for background encoding (if encoder is available)
    pub(super) fn queue_folder_for_encoding(&self, folder: &crate::core::MusicFolder) {
        if let Some(ref encoder) = self.background_encoder {
            encoder.add_folder(folder.clone());
        }
    }

    /// Notify encoder that a folder was removed
    pub(super) fn notify_folder_removed(&self, folder: &crate::core::MusicFolder) {
        if let Some(ref encoder) = self.background_encoder {
            encoder.remove_folder(&folder.id);
        }
    }

    /// Notify encoder that folders were reordered
    #[allow(dead_code)]
    pub(super) fn notify_folders_reordered(&self) {
        if let Some(ref encoder) = self.background_encoder {
            encoder.folders_reordered();
        }
    }

    /// Get the conversion status of a specific folder
    #[allow(dead_code)]
    pub fn get_folder_conversion_status(&self, folder_id: &FolderId) -> FolderConversionStatus {
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
                let (files_completed, files_total) = guard
                    .active_progress
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

    /// Get the list of encoded folder IDs (from background encoder state OR folder status)
    pub(super) fn get_encoded_folder_ids(&self) -> Vec<FolderId> {
        let mut encoded_ids: Vec<FolderId> = Vec::new();

        // Get from encoder's completed map
        if let Some(ref encoder) = self.background_encoder {
            let state = encoder.get_state();
            let guard = state.lock().unwrap();
            encoded_ids.extend(guard.completed.keys().cloned());
        }

        // Also include folders with Converted status (e.g., loaded from bundle)
        for folder in &self.folders {
            if matches!(
                folder.conversion_status,
                FolderConversionStatus::Converted { .. }
            ) {
                if !encoded_ids.contains(&folder.id) {
                    encoded_ids.push(folder.id.clone());
                }
            }
        }

        encoded_ids
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
                    || matches!(
                        folder.conversion_status,
                        FolderConversionStatus::Converted { .. }
                    )
            })
        } else {
            // In legacy mode, check folder status directly
            self.folders.iter().all(|folder| {
                matches!(
                    folder.conversion_status,
                    FolderConversionStatus::Converted { .. }
                )
            })
        }
    }

    /// Poll encoder events and handle them
    ///
    /// Returns true if any events were processed (useful for knowing if UI needs refresh)
    pub(super) fn poll_encoder_events(&mut self) -> bool {
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
                EncoderEvent::FolderProgress {
                    id,
                    files_completed,
                    files_total,
                } => {
                    // Update progress in encoder state for UI rendering
                    if let Some(ref encoder) = self.background_encoder {
                        let state = encoder.get_state();
                        let mut guard = state.lock().unwrap();
                        guard
                            .active_progress
                            .insert(id.clone(), (files_completed, files_total));
                    }
                    println!(
                        "Encoding progress: {:?} {}/{}",
                        id, files_completed, files_total
                    );
                }
                EncoderEvent::FolderCompleted {
                    id,
                    output_dir,
                    output_size,
                    lossless_bitrate,
                } => {
                    println!(
                        "Encoding complete: {:?} -> {:?} ({} bytes, bitrate: {:?})",
                        id, output_dir, output_size, lossless_bitrate
                    );

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
                EncoderEvent::BitrateRecalculated {
                    new_bitrate,
                    reencode_needed,
                } => {
                    println!(
                        "Bitrate recalculated to {} kbps, {} folders need re-encoding",
                        new_bitrate,
                        reencode_needed.len()
                    );
                    // Reset conversion status for folders that need re-encoding
                    for folder in &mut self.folders {
                        if reencode_needed.contains(&folder.id) {
                            folder.conversion_status =
                                crate::core::FolderConversionStatus::NotConverted;
                        }
                    }
                    // Invalidate ISO state - output files are being regenerated
                    self.iso_state = None;
                    self.iso_generation_attempted = false;
                    // Clear the pending flag now that recalculation command has been processed
                    self.bitrate_recalc_pending = false;
                }
                EncoderEvent::PhaseTransition {
                    phase,
                    measured_lossy_size,
                    optimal_bitrate,
                } => {
                    println!(
                        "Phase transition to {:?}: measured lossy size = {} MB, optimal lossless bitrate = {} kbps",
                        phase,
                        measured_lossy_size / 1_000_000,
                        optimal_bitrate
                    );
                    // Update the last calculated bitrate to reflect the optimized value
                    self.last_calculated_bitrate = Some(optimal_bitrate);
                    // Invalidate ISO (will be regenerated after pass 2 completes)
                    self.iso_state = None;
                    self.iso_generation_attempted = false;
                }
            }
        }

        events_processed
    }

    /// Start a polling loop that updates encoder events and triggers ISO generation
    ///
    /// This polls the encoder event channel and updates folder conversion status.
    /// When all folders are encoded, it triggers automatic ISO generation.
    pub(super) fn start_encoder_event_polling(cx: &mut Context<Self>) {
        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                loop {
                    let cx_for_after_await = async_cx.clone();

                    // Wait 100ms between updates (encoder events don't need to be as responsive)
                    Timer::after(Duration::from_millis(100)).await;

                    // Poll encoder events and check if ISO should be generated
                    let should_continue = this
                        .update(&mut async_cx, |this, cx| {
                            // Poll any encoder events
                            let had_events = this.poll_encoder_events();

                            // Poll for volume label updates from the dialog
                            let label_updated = this.poll_volume_label();

                            // Poll for bitrate override dialog result
                            let bitrate_updated = this.poll_bitrate_override();

                            // Check for debounced bitrate recalculation
                            this.check_debounced_bitrate_recalculation();

                            // Check if we should auto-generate ISO
                            if this.maybe_generate_iso(cx) {
                                // ISO generation was triggered
                                println!("Auto-ISO generation triggered");
                            }

                            // Refresh UI if we had events or updates
                            if had_events || label_updated || bitrate_updated {
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
}
