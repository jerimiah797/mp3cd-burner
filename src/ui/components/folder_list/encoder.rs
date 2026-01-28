//! Background encoder management for FolderList
//!
//! Handles background encoding, event polling, and encoder state queries.

use std::time::Duration;

use gpui::{AsyncApp, Context, Timer, WeakEntity};

use crate::conversion::{
    EncoderEvent, EncodingPhase, OutputManager,
    SimpleEncoderHandle, start_simple_encoder, verify_ffmpeg,
};
use std::sync::Arc;
use crate::core::{FolderConversionStatus, FolderId};

use super::FolderList;

impl FolderList {
    /// Initialize the background encoder for immediate folder conversion
    ///
    /// This should be called after construction when background encoding is desired.
    /// If not called, folders will only be converted when "Burn" is clicked (legacy mode).
    /// Returns a clone of the encoder handle so it can be stored as a global.
    pub fn enable_background_encoding(&mut self) -> Result<SimpleEncoderHandle, String> {
        // Create the output manager first
        let output_manager = Arc::new(OutputManager::new()?);

        // Clean up old sessions from previous runs
        output_manager.cleanup_old_sessions()?;

        // Get ffmpeg path
        let ffmpeg_path = verify_ffmpeg()?;

        // Create the simple encoder
        let (handle, event_rx) = start_simple_encoder(output_manager.clone(), ffmpeg_path);

        // Store the handle, event receiver, and output manager
        let handle_clone = handle.clone();
        self.simple_encoder = Some(handle);
        self.encoder_event_rx = Some(event_rx);
        self.output_manager = Some((*output_manager).clone());

        log::debug!(
            "Simple encoder enabled, session: {:?}",
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

    /// Set the encoder handle (for use from async context)
    #[allow(dead_code)]
    pub fn set_simple_encoder(&mut self, handle: SimpleEncoderHandle) {
        self.simple_encoder = Some(handle);
    }

    /// Check if background encoding is available
    #[allow(dead_code)]
    pub fn has_simple_encoder(&self) -> bool {
        self.simple_encoder.is_some()
    }

    /// Get the output manager if available
    #[allow(dead_code)]
    pub fn output_manager(&self) -> Option<&OutputManager> {
        self.output_manager.as_ref()
    }

    /// Get the current encoding phase (for UI display logic)
    #[allow(dead_code)]
    pub fn get_encoding_phase(&self) -> EncodingPhase {
        if let Some(ref encoder) = self.simple_encoder {
            encoder.get_state().get_phase()
        } else {
            EncodingPhase::Idle
        }
    }

    /// Check if the bitrate is preliminary (will be recalculated after lossy encoding)
    ///
    /// Returns true when:
    /// - We're in LossyPass (actively encoding lossy files), OR
    /// - We're in Idle with lossless files to encode (before encoding starts)
    ///
    /// Returns false during LosslessPass (we have the final optimized bitrate).
    pub fn is_bitrate_preliminary(&self) -> bool {
        if let Some(ref encoder) = self.simple_encoder {
            let phase = encoder.get_state().get_phase();
            match phase {
                // During LossyPass, bitrate is always preliminary
                EncodingPhase::LossyPass => true,
                // During Idle, check if any folders have lossless files
                EncodingPhase::Idle => {
                    let folders = encoder.get_shared_folders();
                    let guard = folders.lock().unwrap();
                    guard.iter().any(|f| f.has_lossless_files())
                }
                // During LosslessPass or Complete, we have the final bitrate
                EncodingPhase::LosslessPass | EncodingPhase::Complete => false,
            }
        } else {
            false
        }
    }

    /// Update encoder's embed_album_art setting
    #[allow(dead_code)]
    pub fn set_embed_album_art(&self, embed: bool) {
        if let Some(ref encoder) = self.simple_encoder {
            log::debug!("[FolderList] Sending embed_album_art={} to encoder", embed);
            encoder.set_embed_album_art(embed);
        } else {
            log::debug!("[FolderList] WARNING: No encoder to send embed_album_art to!");
        }
    }

    /// Queue a folder for background encoding (if encoder is available)
    pub(super) fn queue_folder_for_encoding(&self, folder: &crate::core::MusicFolder) {
        if let Some(ref encoder) = self.simple_encoder {
            encoder.add_folder(folder.clone());
        }
    }

    /// Notify encoder that a folder was removed
    pub(super) fn notify_folder_removed(&self, folder: &crate::core::MusicFolder) {
        if let Some(ref encoder) = self.simple_encoder {
            encoder.remove_folder(&folder.id);
        }
    }

    /// Notify encoder that folders were reordered
    #[allow(dead_code)]
    pub(super) fn notify_folders_reordered(&self) {
        if let Some(ref encoder) = self.simple_encoder {
            encoder.folders_reordered();
        }
    }

    /// Get the conversion status of a specific folder
    #[allow(dead_code)]
    pub fn get_folder_conversion_status(&self, folder_id: &FolderId) -> FolderConversionStatus {
        // Simple encoder tracks status via folder.conversion_status updated by events
        if let Some(folder) = self.folders.iter().find(|f| &f.id == folder_id) {
            folder.conversion_status.clone()
        } else {
            FolderConversionStatus::NotConverted
        }
    }

    /// Get the list of encoded folder IDs
    pub(super) fn get_encoded_folder_ids(&self) -> Vec<FolderId> {
        // Get folders with Converted status
        self.folders
            .iter()
            .filter(|f| matches!(f.conversion_status, FolderConversionStatus::Converted { .. }))
            .map(|f| f.id.clone())
            .collect()
    }

    /// Check if all folders are ready (converted) for burning
    pub fn all_folders_converted(&self) -> bool {
        if self.folders.is_empty() {
            return false;
        }

        // Check folder status directly - simple encoder updates this via events
        self.folders.iter().all(|folder| {
            matches!(
                folder.conversion_status,
                FolderConversionStatus::Converted { .. }
            )
        })
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
                    // Update folder to "Converting" status
                    if let Some(folder) = self.folders.iter_mut().find(|f| f.id == id) {
                        folder.conversion_status = FolderConversionStatus::Converting {
                            files_completed: 0,
                            files_total,
                        };
                    }
                    log::debug!("Encoding started: {:?} ({} files)", id, files_total);
                }
                EncoderEvent::FolderProgress {
                    id,
                    files_completed,
                    files_total,
                } => {
                    // Update folder progress
                    if let Some(folder) = self.folders.iter_mut().find(|f| f.id == id) {
                        folder.conversion_status = FolderConversionStatus::Converting {
                            files_completed,
                            files_total,
                        };
                    }
                    log::debug!(
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
                    log::debug!(
                        "Encoding complete: {:?} -> {:?} ({} bytes, bitrate: {:?})",
                        id, output_dir, output_size, lossless_bitrate
                    );

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
                    log::error!("Encoding failed: {:?} - {}", id, error);
                }
                EncoderEvent::FolderCancelled(id) => {
                    log::debug!("Encoding cancelled: {:?}", id);
                }
                EncoderEvent::BitrateRecalculated {
                    new_bitrate,
                    reencode_needed,
                } => {
                    log::debug!(
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
                    log::debug!(
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
                    // Returns (should_continue, had_changes)
                    let result = this
                        .update(&mut async_cx, |this, cx| {
                            let mut had_changes = false;

                            // Poll any encoder events
                            if this.poll_encoder_events() {
                                had_changes = true;
                            }

                            // Poll for volume label updates from the dialog
                            if this.poll_volume_label() {
                                had_changes = true;
                            }

                            // Poll for bitrate override dialog result
                            if this.poll_bitrate_override() {
                                had_changes = true;
                            }

                            // Check for debounced bitrate recalculation
                            if this.check_debounced_bitrate_recalculation() {
                                had_changes = true;
                            }

                            // Check if we should auto-generate ISO
                            if this.maybe_generate_iso(cx) {
                                log::debug!("Auto-ISO generation triggered");
                                had_changes = true;
                            }

                            // Only notify UI if there were actual changes
                            if had_changes {
                                cx.notify();
                            }

                            // Continue polling as long as we have a background encoder
                            (this.simple_encoder.is_some(), had_changes)
                        })
                        .unwrap_or((false, false));

                    let (should_continue, _had_changes) = result;

                    if !should_continue {
                        break;
                    }

                    // Note: cx.notify() inside update() is sufficient to trigger re-render
                    // Calling refresh() here would refresh ALL windows, causing unnecessary
                    // re-renders of the track editor and other windows
                    async_cx = cx_for_after_await;
                }
            }
        })
        .detach();
    }
}
