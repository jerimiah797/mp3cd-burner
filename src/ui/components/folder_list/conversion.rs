//! Conversion and burn operations for FolderList
//!
//! Handles bitrate calculation, burn workflows, and progress polling.

use std::time::Duration;

use gpui::{AnyWindowHandle, AsyncApp, Context, PromptLevel, Timer, WeakEntity, Window};

use crate::burning::IsoState;
use crate::conversion::{MultipassEstimate, calculate_multipass_bitrate};
use crate::core::{AppSettings, BurnStage, ConversionState};
use crate::ui::components::BitrateOverrideDialog;

use super::{FolderList, PendingBurnAction};

impl FolderList {
    /// Check and execute any pending burn action
    ///
    /// This should be called from the render loop where we have window access.
    /// Returns true if an action was triggered.
    pub(super) fn check_pending_burn_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
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

    /// Calculate the optimal bitrate to fit on a 700MB CD
    ///
    /// Uses multi-pass-aware calculation:
    /// - MP3s are copied (exact size)
    /// - Lossy files transcoded at source bitrate
    /// - Lossless files get remaining space
    ///
    /// Returns the full estimate with bitrate and display logic
    pub fn calculated_bitrate_estimate(&self) -> Option<MultipassEstimate> {
        if self.folders.is_empty() {
            return None;
        }

        // Collect all audio files from cached folder data
        let all_files: Vec<_> = self
            .folders
            .iter()
            .flat_map(|f| f.audio_files.iter().cloned())
            .collect();

        if all_files.is_empty() {
            return None;
        }

        // Use multi-pass-aware calculation
        let mut estimate = calculate_multipass_bitrate(&all_files);

        // If we have an optimized bitrate from pass 2 (stored in last_calculated_bitrate),
        // use that instead of the preliminary estimate. This happens after the phase
        // transition when we've measured actual lossy sizes.
        if let Some(optimized_bitrate) = self.last_calculated_bitrate {
            // Only use the optimized bitrate if we're not in preliminary mode anymore
            // (i.e., pass 2 has started or completed)
            if !self.is_bitrate_preliminary() {
                estimate.target_bitrate = optimized_bitrate;
            }
        }

        Some(estimate)
    }

    /// Get the target bitrate for encoding
    ///
    /// If a manual override is set, returns that value.
    /// Otherwise returns the automatically calculated bitrate.
    pub fn calculated_bitrate(&self) -> u32 {
        // If manual override is set, use it
        if let Some(override_bitrate) = self.manual_bitrate_override {
            return override_bitrate;
        }

        // Otherwise calculate automatically
        self.calculated_bitrate_estimate()
            .map(|e| e.target_bitrate)
            .unwrap_or(320)
    }

    /// Show the bitrate override dialog
    pub fn show_bitrate_override_dialog(&mut self, cx: &mut Context<Self>) {
        // Get current effective bitrate (either override or calculated)
        let current_bitrate = self.calculated_bitrate();
        // Get the auto-calculated bitrate for reference
        let calculated_bitrate = self
            .calculated_bitrate_estimate()
            .map(|e| e.target_bitrate)
            .unwrap_or(320);

        let (tx, rx) = std::sync::mpsc::channel();
        self.pending_bitrate_rx = Some(rx);

        BitrateOverrideDialog::open(
            cx,
            current_bitrate,
            calculated_bitrate,
            move |new_bitrate| {
                let _ = tx.send(new_bitrate);
            },
        );
    }

    /// Poll for bitrate override dialog result
    ///
    /// Returns true if a new bitrate was received and applied.
    pub(super) fn poll_bitrate_override(&mut self) -> bool {
        if let Some(ref rx) = self.pending_bitrate_rx
            && let Ok(new_bitrate) = rx.try_recv() {
                println!("Manual bitrate override: {} kbps", new_bitrate);

                self.manual_bitrate_override = Some(new_bitrate);
                self.pending_bitrate_rx = None;

                // Set flag to prevent ISO generation until recalculation completes
                self.bitrate_recalc_pending = true;

                // Trigger re-encoding at new bitrate
                // This handles all folders in the encoder's completed map (including bundle folders)
                if let Some(ref encoder) = self.simple_encoder {
                    encoder.recalculate_bitrate(new_bitrate);
                }

                // Reset lossless folder statuses immediately to prevent ISO race condition
                // (The BitrateRecalculated event will also do this, but it comes later)
                for folder in &mut self.folders {
                    if let crate::core::FolderConversionStatus::Converted {
                        lossless_bitrate: Some(br),
                        ..
                    } = folder.conversion_status
                        && br != new_bitrate {
                            folder.conversion_status =
                                crate::core::FolderConversionStatus::NotConverted;
                        }
                }

                // Invalidate ISO state - output files are being regenerated
                self.iso_state = None;
                self.iso_generation_attempted = false;

                return true;
            }
        false
    }

    /// Check if debounce period has passed and trigger bitrate recalculation
    ///
    /// This is called from the encoder polling loop. When folder list changes:
    /// 1. Wait 500ms (debounce) to let rapid additions settle
    /// 2. Calculate new target bitrate
    /// 3. If bitrate changed, send recalculate command to encoder
    pub(super) fn check_debounced_bitrate_recalculation(&mut self) {
        const DEBOUNCE_MS: u64 = 500;

        // Check if we have a pending change that's old enough
        let should_recalculate = match self.last_folder_change {
            Some(change_time) => change_time.elapsed() >= Duration::from_millis(DEBOUNCE_MS),
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

        println!(
            "Bitrate recalculated: {:?} -> {} kbps",
            self.last_calculated_bitrate
                .map(|b| format!("{}", b))
                .unwrap_or_else(|| "None".to_string()),
            new_bitrate
        );

        // Send recalculation command to background encoder
        // This handles all folders in the encoder's completed map (including bundle folders)
        if let Some(ref encoder) = self.simple_encoder {
            encoder.recalculate_bitrate(new_bitrate);
        }
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
    pub(super) fn run_conversion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let encoder_handle = match &self.simple_encoder {
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
        let folders: Vec<_> = self.folders.to_vec();
        let volume_label = self.volume_label.clone();

        // Spawn background thread to execute the full burn workflow
        std::thread::spawn(move || {
            crate::burning::execute_full_burn(
                state,
                encoder_handle,
                output_manager,
                folders,
                simulate_burn,
                volume_label,
            );
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
    pub(super) fn burn_existing_iso(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn start_progress_polling(
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
                    Timer::after(Duration::from_millis(50)).await;

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
                        if should_update
                            && let Ok(iso_state) = IsoState::new(path, &folder_list.folders) {
                                folder_list.iso_state = Some(iso_state);
                                println!("ISO state saved - ready for Burn/Burn Another");
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

                    // Show completion prompt - await the future so it displays
                    use gpui::AppContext;
                    if let Ok(prompt_future) =
                        async_cx.update_window(window_handle, |_, window, cx| {
                            window.prompt(
                                PromptLevel::Info,
                                "Burn Complete",
                                Some("The CD has been burned successfully."),
                                &["OK"],
                                cx,
                            )
                        })
                    {
                        let _ = prompt_future.await;
                    }
                }
            }
        })
        .detach();
    }
}
