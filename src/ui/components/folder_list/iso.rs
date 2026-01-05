//! ISO state management for FolderList
//!
//! Handles ISO state queries, burn-another logic, and auto-generation.

use std::time::Duration;

use gpui::{AsyncApp, Context, Timer, WeakEntity};

use crate::burning::{determine_iso_action, IsoAction, IsoGenerationCheck, IsoState};
use crate::core::{ConversionState, MusicFolder};

use super::FolderList;

impl FolderList {
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

    /// Get the ISO size in MB (decimal, to match Finder and CD labels)
    pub fn iso_size_mb(&self) -> Option<f64> {
        self.iso_state.as_ref().map(|iso| iso.size_bytes as f64 / 1_000_000.0)
    }

    /// Determine what action is needed for the current burn request
    #[allow(dead_code)]
    pub fn determine_burn_action(&self) -> IsoAction {
        let encoded_ids = self.get_encoded_folder_ids();
        determine_iso_action(self.iso_state.as_ref(), &self.folders, &encoded_ids)
    }

    /// Check if ISO needs to be generated and do it if ready
    ///
    /// This should be called periodically to auto-generate ISO when all folders are encoded.
    /// Returns true if ISO generation was triggered.
    pub(super) fn maybe_generate_iso(&mut self, cx: &mut Context<Self>) -> bool {
        // Don't generate ISO if a bitrate recalculation is pending
        // (command sent but not yet processed by encoder)
        if self.bitrate_recalc_pending {
            return false;
        }

        // Don't generate ISO if the encoder has pending work (queue or active folders)
        // This prevents race conditions where ISO is generated before re-encoding completes
        if let Some(ref encoder) = self.background_encoder {
            let state = encoder.get_state();
            let guard = state.lock().unwrap();
            if !guard.queue.is_empty() || !guard.active.is_empty() {
                return false;
            }
        }

        // Don't generate ISO while profile import is in progress
        // (folders are still being loaded from bundle)
        // Also check for pending folders that haven't been drained yet
        if self.import_state.is_importing() || self.import_state.has_pending_folders() {
            return false;
        }

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
    pub(super) fn start_iso_creation_polling(
        state: ConversionState,
        folders: Vec<MusicFolder>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                loop {
                    let cx_for_after_await = async_cx.clone();
                    Timer::after(Duration::from_millis(100)).await;

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
                    let _ = this.update(&mut async_cx, |folder_list, cx| {
                        if let Ok(iso_state) = IsoState::new(path.clone(), &folders) {
                            println!("ISO size: {} bytes ({:.1} MB)",
                                iso_state.size_bytes,
                                iso_state.size_bytes as f64 / 1_000_000.0);
                            folder_list.iso_state = Some(iso_state);
                            println!("ISO state saved - ready for Burn");
                            cx.notify(); // Ensure UI updates with new ISO size
                        }
                    });
                }

                let _ = async_cx.refresh();
            }
        })
        .detach();
    }
}
