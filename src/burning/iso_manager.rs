//! ISO manager - orchestrates ISO generation workflow
//!
//! This module extracts ISO generation logic from the folder list component,
//! handling the decision of when to generate and the actual generation process.

use std::path::PathBuf;

use super::iso::create_iso;
use crate::conversion::OutputManager;
use crate::core::{BurnStage, ConversionState, MusicFolder};

/// Conditions required for ISO generation
pub struct IsoGenerationCheck {
    /// Whether we already have a valid ISO
    pub has_valid_iso: bool,
    /// Whether ISO generation was already attempted
    pub already_attempted: bool,
    /// Whether there are folders to process
    pub has_folders: bool,
    /// Whether all folders are converted
    pub all_converted: bool,
    /// Whether a conversion/burn is in progress
    pub is_busy: bool,
}

impl IsoGenerationCheck {
    /// Check if ISO generation should proceed
    pub fn should_generate(&self) -> bool {
        !self.has_valid_iso
            && !self.already_attempted
            && self.has_folders
            && self.all_converted
            && !self.is_busy
    }
}

/// Generate an ISO from the given folders
///
/// This function:
/// 1. Creates a staging directory with symlinks to encoded folders
/// 2. Runs hdiutil to create the ISO
/// 3. Returns the path to the created ISO
///
/// This is a blocking operation that should be run in a background thread.
pub fn generate_iso(
    output_manager: &OutputManager,
    folders: &[MusicFolder],
    volume_label: &str,
    state: &ConversionState,
) -> Result<PathBuf, String> {
    // Mark as creating ISO
    state.set_stage(BurnStage::CreatingIso);

    // Create staging directory with symlinks
    let staging_dir = output_manager.create_iso_staging(folders)?;
    log::info!("ISO staging directory: {:?}", staging_dir);

    // Create ISO from staging directory
    let result = create_iso(&staging_dir, volume_label)?;
    log::info!("ISO created successfully: {:?}", result.iso_path);

    // Store ISO path in conversion state
    let iso_path = result.iso_path.clone();
    *state.iso_path.lock().unwrap() = Some(iso_path.clone());

    Ok(iso_path)
}

/// Spawn ISO generation in a background thread
///
/// This is the recommended way to generate an ISO - it handles threading
/// and state management automatically.
///
/// Returns immediately. The ConversionState will be updated as generation progresses.
pub fn spawn_iso_generation(
    output_manager: OutputManager,
    folders: Vec<MusicFolder>,
    state: ConversionState,
    volume_label: String,
) {
    // Reset state for ISO generation
    state.reset(0);
    state.set_stage(BurnStage::CreatingIso);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(async {
            match generate_iso(&output_manager, &folders, &volume_label, &state) {
                Ok(_result) => {
                    state.set_stage(BurnStage::Complete);
                }
                Err(e) => {
                    log::error!("ISO generation failed: {}", e);
                    state.set_stage(BurnStage::Complete);
                }
            }
            state.finish();
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iso_generation_check_should_generate() {
        // All conditions met - should generate
        let check = IsoGenerationCheck {
            has_valid_iso: false,
            already_attempted: false,
            has_folders: true,
            all_converted: true,
            is_busy: false,
        };
        assert!(check.should_generate());
    }

    #[test]
    fn test_iso_generation_check_has_valid_iso() {
        // Already have valid ISO - should not generate
        let check = IsoGenerationCheck {
            has_valid_iso: true,
            already_attempted: false,
            has_folders: true,
            all_converted: true,
            is_busy: false,
        };
        assert!(!check.should_generate());
    }

    #[test]
    fn test_iso_generation_check_already_attempted() {
        // Already attempted - should not retry
        let check = IsoGenerationCheck {
            has_valid_iso: false,
            already_attempted: true,
            has_folders: true,
            all_converted: true,
            is_busy: false,
        };
        assert!(!check.should_generate());
    }

    #[test]
    fn test_iso_generation_check_no_folders() {
        // No folders - nothing to generate
        let check = IsoGenerationCheck {
            has_valid_iso: false,
            already_attempted: false,
            has_folders: false,
            all_converted: true,
            is_busy: false,
        };
        assert!(!check.should_generate());
    }

    #[test]
    fn test_iso_generation_check_not_all_converted() {
        // Still converting - wait
        let check = IsoGenerationCheck {
            has_valid_iso: false,
            already_attempted: false,
            has_folders: true,
            all_converted: false,
            is_busy: false,
        };
        assert!(!check.should_generate());
    }

    #[test]
    fn test_iso_generation_check_is_busy() {
        // Busy with something else - wait
        let check = IsoGenerationCheck {
            has_valid_iso: false,
            already_attempted: false,
            has_folders: true,
            all_converted: true,
            is_busy: true,
        };
        assert!(!check.should_generate());
    }
}
