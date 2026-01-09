//! Burn workflow execution
//!
//! This module handles the execution of burn workflows in background threads.
//! It extracts the core burn logic from the UI layer.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use super::coordinator::{BurnConfig, coordinate_burn};
use super::iso::create_iso;
use crate::conversion::{EncodingPhase, OutputManager, SimpleEncoderHandle};
use crate::core::{BurnStage, ConversionState, MusicFolder};

/// Execute a full burn workflow: wait for conversion, create ISO, burn
///
/// This is a blocking function that should be run in a background thread.
/// It will:
/// 1. Wait for all folders to be converted by the background encoder
/// 2. Create an ISO staging directory with the converted folders
/// 3. Create an ISO from the staging directory
/// 4. Coordinate the burn process (wait for CD, burn, etc.)
///
/// Progress is reported via the ConversionState.
pub fn execute_full_burn(
    state: ConversionState,
    encoder_handle: SimpleEncoderHandle,
    output_manager: OutputManager,
    folders: Vec<MusicFolder>,
    simulate_burn: bool,
    volume_label: String,
) {
    // Wait for all folders to be converted
    loop {
        if state.is_cancelled() {
            log::info!("Burn cancelled while waiting for conversion");
            state.set_stage(BurnStage::Cancelled);
            state.finish();
            return;
        }

        // Check if encoder is idle/complete (no active work)
        let phase = encoder_handle.get_state().get_phase();
        let is_done = matches!(phase, EncodingPhase::Complete | EncodingPhase::Idle);

        // Count folders that have output (converted)
        let completed_count = folders
            .iter()
            .filter(|f| output_manager.get_folder_output_size(&f.id).unwrap_or(0) > 0)
            .count();

        // Update progress
        state.completed.store(completed_count, Ordering::SeqCst);

        if is_done && completed_count == folders.len() {
            log::info!("All folders converted ({} total)", completed_count);
            break;
        }

        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // Create ISO staging directory with symlinks to converted folders
    state.set_stage(BurnStage::CreatingIso);
    log::info!("\n=== Creating ISO image ===");

    let staging_dir = match output_manager.create_iso_staging(&folders) {
        Ok(dir) => {
            log::info!("ISO staging directory: {:?}", dir);
            dir
        }
        Err(e) => {
            log::error!("Failed to create ISO staging: {}", e);
            state.set_stage(BurnStage::Complete);
            state.finish();
            return;
        }
    };

    // Create ISO and burn
    execute_iso_and_burn(state, staging_dir, simulate_burn, volume_label);
}

/// Execute ISO creation and burn
///
/// This is a blocking function that:
/// 1. Creates an ISO from the staging directory
/// 2. Coordinates the burn process
fn execute_iso_and_burn(
    state: ConversionState,
    staging_dir: PathBuf,
    simulate_burn: bool,
    volume_label: String,
) {
    state.set_stage(BurnStage::CreatingIso);
    log::info!("\n=== Creating ISO image ===");

    match create_iso(&staging_dir, &volume_label) {
        Ok(result) => {
            log::info!("ISO created at: {}", result.iso_path.display());
            *state.iso_path.lock().unwrap() = Some(result.iso_path.clone());

            // Coordinate the burn process
            execute_burn(&result.iso_path, &state, simulate_burn);
        }
        Err(e) => {
            log::error!("ISO creation failed: {}", e);
            state.set_stage(BurnStage::Complete);
            state.finish();
        }
    }
}

/// Execute burn of an existing ISO
///
/// This is a blocking function that should be run in a background thread.
/// It coordinates the burn process for an existing ISO file.
pub fn execute_burn_existing(state: ConversionState, iso_path: PathBuf, simulate_burn: bool) {
    execute_burn(&iso_path, &state, simulate_burn);
}

/// Execute the burn coordination
fn execute_burn(iso_path: &Path, state: &ConversionState, simulate_burn: bool) {
    let config = BurnConfig {
        simulate: simulate_burn,
        ..Default::default()
    };

    let result = coordinate_burn(iso_path, state, &config);
    log::info!("Burn coordination result: {:?}", result);
    state.finish();
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
