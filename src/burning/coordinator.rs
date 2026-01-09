//! Burn coordination - handles CD detection, erase approval, and burn execution
//!
//! This module coordinates the burn process:
//! 1. Wait for a usable CD (blank or erasable with user approval)
//! 2. Set up progress tracking with stage transitions
//! 3. Execute the burn
//! 4. Handle results and update state

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::burning::cd::{CdStatus, burn_iso_with_cancel, check_cd_status};
use crate::core::{BurnStage, ConversionState};

/// Configuration for burn coordination
pub struct BurnConfig {
    /// If true, skip actual burning (just simulate)
    pub simulate: bool,
    /// Timeout in seconds for waiting for CD
    pub cd_wait_timeout_secs: u32,
}

impl Default for BurnConfig {
    fn default() -> Self {
        Self {
            simulate: false,
            cd_wait_timeout_secs: 120,
        }
    }
}

/// Result of burn coordination
#[derive(Debug)]
pub enum BurnCoordinationResult {
    /// Burn completed successfully
    Success,
    /// Burn was simulated (no actual burn)
    Simulated,
    /// Burn was cancelled by user
    Cancelled,
    /// No usable CD found within timeout
    NoCdTimeout,
    /// Burn failed with error (string used by Debug impl)
    Error(#[allow(dead_code)] String),
}

/// Coordinate the burn process for an ISO file
///
/// This function handles:
/// - Waiting for a usable CD (blank or user-approved erase)
/// - Stage transitions (WaitingForCd -> Erasing/Burning -> Finishing -> Complete)
/// - Progress tracking
/// - Cancellation
///
/// # Arguments
/// * `iso_path` - Path to the ISO file to burn
/// * `state` - ConversionState for stage updates and cancellation
/// * `config` - Burn configuration (simulate, timeout)
///
/// # Returns
/// * `BurnCoordinationResult` indicating the outcome
pub fn coordinate_burn(
    iso_path: &Path,
    state: &ConversionState,
    config: &BurnConfig,
) -> BurnCoordinationResult {
    let cancel_token = state.cancel_requested.clone();

    // Check for simulate mode first
    if config.simulate {
        log::info!("\n=== SIMULATED BURN ===");
        log::info!("Would burn ISO: {}", iso_path.display());
        state.set_stage(BurnStage::Complete);
        return BurnCoordinationResult::Simulated;
    }

    // Wait for usable CD
    log::info!("\n=== Waiting for blank CD ===");
    state.set_stage(BurnStage::WaitingForCd);

    let wait_result = wait_for_cd(state, &cancel_token, config.cd_wait_timeout_secs);
    let erase_first = match wait_result {
        WaitForCdResult::BlankCd => false,
        WaitForCdResult::ErasableCdApproved => true,
        WaitForCdResult::Cancelled => {
            state.set_stage(BurnStage::Cancelled);
            return BurnCoordinationResult::Cancelled;
        }
        WaitForCdResult::Timeout => {
            log::info!("No usable CD found after timeout");
            state.set_stage(BurnStage::Complete);
            return BurnCoordinationResult::NoCdTimeout;
        }
    };

    // Start burning
    if erase_first {
        state.set_stage(BurnStage::Erasing);
        log::info!("\n=== Erasing and Burning CD ===");
    } else {
        state.set_stage(BurnStage::Burning);
        log::info!("\n=== Burning CD ===");
    }

    // Set up progress callback with stage transition logic
    let progress_callback = create_progress_callback(state.clone(), erase_first);

    // Execute burn
    match burn_iso_with_cancel(
        iso_path,
        Some(progress_callback),
        Some(cancel_token),
        erase_first,
    ) {
        Ok(()) => {
            log::info!("CD burned successfully!");
            state.set_stage(BurnStage::Complete);
            BurnCoordinationResult::Success
        }
        Err(e) if e.contains("cancelled") => {
            log::info!("Burn was cancelled");
            state.set_stage(BurnStage::Cancelled);
            BurnCoordinationResult::Cancelled
        }
        Err(e) => {
            log::error!("Burn failed: {}", e);
            state.set_stage(BurnStage::Complete);
            BurnCoordinationResult::Error(e)
        }
    }
}

/// Result of waiting for CD
enum WaitForCdResult {
    /// Found blank CD
    BlankCd,
    /// Found erasable CD and user approved erase
    ErasableCdApproved,
    /// User cancelled
    Cancelled,
    /// Timeout waiting for CD
    Timeout,
}

/// Wait for a usable CD (blank or user-approved erasable)
fn wait_for_cd(
    state: &ConversionState,
    cancel_token: &Arc<std::sync::atomic::AtomicBool>,
    timeout_secs: u32,
) -> WaitForCdResult {
    for _ in 0..timeout_secs {
        // Check for cancellation
        if cancel_token.load(Ordering::SeqCst) {
            log::info!("Cancelled while waiting for CD");
            return WaitForCdResult::Cancelled;
        }

        match check_cd_status() {
            Ok(CdStatus::Blank) => {
                log::info!("Blank CD detected");
                return WaitForCdResult::BlankCd;
            }
            Ok(CdStatus::ErasableWithData) => {
                log::info!("Erasable disc (CD-RW) with data detected");
                state.set_stage(BurnStage::ErasableDiscDetected);

                // Wait for user to approve erase or cancel
                match wait_for_erase_approval(state, cancel_token) {
                    Some(true) => {
                        log::info!("User approved erase - will erase and burn");
                        return WaitForCdResult::ErasableCdApproved;
                    }
                    Some(false) | None => {
                        return WaitForCdResult::Cancelled;
                    }
                }
            }
            Ok(CdStatus::NonErasable) => {
                log::info!("Non-erasable disc detected - please insert a blank disc");
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            Ok(CdStatus::NoDisc) => {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Err(e) => {
                log::error!("Error checking CD: {}", e);
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }

    WaitForCdResult::Timeout
}

/// Wait for user to approve erase
/// Returns Some(true) if approved, Some(false) if explicitly rejected, None if cancelled
fn wait_for_erase_approval(
    state: &ConversionState,
    cancel_token: &Arc<std::sync::atomic::AtomicBool>,
) -> Option<bool> {
    loop {
        if cancel_token.load(Ordering::SeqCst) {
            log::info!("Cancelled while waiting for erase approval");
            return None;
        }
        if state.erase_approved.load(Ordering::SeqCst) {
            return Some(true);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Create progress callback that handles stage transitions
fn create_progress_callback(state: ConversionState, is_erasing: bool) -> Box<dyn Fn(i32) + Send> {
    let last_progress = Arc::new(AtomicI32::new(-1));

    Box::new(move |progress: i32| {
        let current_stage = state.get_stage();
        let prev = last_progress.load(Ordering::SeqCst);

        // Handle -1 (indeterminate) values
        if progress < 0 {
            // If we were at high progress (>=95) in Burning stage, switch to Finishing
            if prev >= 95 && current_stage == BurnStage::Burning {
                state.set_stage(BurnStage::Finishing);
            }
            return;
        }

        // Store current progress for next comparison
        last_progress.store(progress, Ordering::SeqCst);

        // Detect phase transition: progress was high (>50) and now low (<20)
        // This indicates erase completed and burn started
        if is_erasing && prev > 50 && progress < 20 && current_stage == BurnStage::Erasing {
            state.set_stage(BurnStage::Burning);
        }

        state.set_burn_progress(progress);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_burn_config_default() {
        let config = BurnConfig::default();
        assert!(!config.simulate);
        assert_eq!(config.cd_wait_timeout_secs, 120);
    }
}
