//! Background encoder events and types
//!
//! This module provides shared types used by the simple encoder system.

use std::path::PathBuf;

use crate::core::FolderId;

/// Encoding phase for two-pass optimization
///
/// Two-pass encoding maximizes CD utilization:
/// - Pass 1: Encode lossy files at source bitrate (size is predictable)
/// - Measure actual sizes after pass 1
/// - Pass 2: Encode lossless files at optimized bitrate based on remaining space
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingPhase {
    /// Initial state - no encoding in progress
    Idle,
    /// Pass 1: Encoding lossy files, lossless files wait
    LossyPass,
    /// Pass 2: All lossy complete, encoding lossless at optimized bitrate
    LosslessPass,
    /// All encoding complete
    Complete,
}

impl Default for EncodingPhase {
    fn default() -> Self {
        Self::Idle
    }
}

/// Events emitted by the background encoder
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants are matched in encoder.rs event handling
pub enum EncoderEvent {
    /// Started encoding a folder
    FolderStarted { id: FolderId, files_total: usize },
    /// Progress update for a folder
    FolderProgress {
        id: FolderId,
        files_completed: usize,
        files_total: usize,
    },
    /// Folder encoding completed successfully
    FolderCompleted {
        id: FolderId,
        output_dir: PathBuf,
        output_size: u64,
        lossless_bitrate: Option<u32>,
    },
    /// Folder encoding failed
    FolderFailed { id: FolderId, error: String },
    /// Folder was cancelled (removed mid-encoding)
    FolderCancelled(FolderId),
    /// Bitrate was recalculated, some folders need re-encoding
    BitrateRecalculated {
        new_bitrate: u32,
        reencode_needed: Vec<FolderId>,
    },
    /// Encoding phase changed (pass 1 -> pass 2)
    PhaseTransition {
        phase: EncodingPhase,
        measured_lossy_size: u64,
        optimal_bitrate: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoding_phase_default() {
        assert_eq!(EncodingPhase::default(), EncodingPhase::Idle);
    }

    #[test]
    fn test_encoding_phase_eq() {
        assert_eq!(EncodingPhase::LossyPass, EncodingPhase::LossyPass);
        assert_ne!(EncodingPhase::LossyPass, EncodingPhase::LosslessPass);
    }
}
