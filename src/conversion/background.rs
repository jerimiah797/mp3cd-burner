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

    #[test]
    fn test_encoding_phase_clone() {
        let phase = EncodingPhase::Complete;
        let cloned = phase.clone();
        assert_eq!(phase, cloned);
    }

    #[test]
    fn test_encoding_phase_copy() {
        let phase = EncodingPhase::LosslessPass;
        let copied = phase;
        assert_eq!(phase, copied);
    }

    #[test]
    fn test_encoding_phase_debug() {
        let phase = EncodingPhase::LossyPass;
        let debug_str = format!("{:?}", phase);
        assert!(debug_str.contains("LossyPass"));
    }

    #[test]
    fn test_encoder_event_folder_started() {
        let event = EncoderEvent::FolderStarted {
            id: FolderId("test".to_string()),
            files_total: 10,
        };
        match event {
            EncoderEvent::FolderStarted { id, files_total } => {
                assert_eq!(id.0, "test");
                assert_eq!(files_total, 10);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_encoder_event_folder_progress() {
        let event = EncoderEvent::FolderProgress {
            id: FolderId("test".to_string()),
            files_completed: 5,
            files_total: 10,
        };
        match event {
            EncoderEvent::FolderProgress {
                id,
                files_completed,
                files_total,
            } => {
                assert_eq!(id.0, "test");
                assert_eq!(files_completed, 5);
                assert_eq!(files_total, 10);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_encoder_event_folder_completed() {
        let event = EncoderEvent::FolderCompleted {
            id: FolderId("test".to_string()),
            output_dir: PathBuf::from("/tmp/output"),
            output_size: 1_000_000,
            lossless_bitrate: Some(256),
        };
        match event {
            EncoderEvent::FolderCompleted {
                id,
                output_dir,
                output_size,
                lossless_bitrate,
            } => {
                assert_eq!(id.0, "test");
                assert_eq!(output_dir, PathBuf::from("/tmp/output"));
                assert_eq!(output_size, 1_000_000);
                assert_eq!(lossless_bitrate, Some(256));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_encoder_event_folder_failed() {
        let event = EncoderEvent::FolderFailed {
            id: FolderId("test".to_string()),
            error: "Something went wrong".to_string(),
        };
        match event {
            EncoderEvent::FolderFailed { id, error } => {
                assert_eq!(id.0, "test");
                assert!(error.contains("wrong"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_encoder_event_folder_cancelled() {
        let event = EncoderEvent::FolderCancelled(FolderId("test".to_string()));
        match event {
            EncoderEvent::FolderCancelled(id) => {
                assert_eq!(id.0, "test");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_encoder_event_bitrate_recalculated() {
        let event = EncoderEvent::BitrateRecalculated {
            new_bitrate: 192,
            reencode_needed: vec![FolderId("a".to_string()), FolderId("b".to_string())],
        };
        match event {
            EncoderEvent::BitrateRecalculated {
                new_bitrate,
                reencode_needed,
            } => {
                assert_eq!(new_bitrate, 192);
                assert_eq!(reencode_needed.len(), 2);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_encoder_event_phase_transition() {
        let event = EncoderEvent::PhaseTransition {
            phase: EncodingPhase::LosslessPass,
            measured_lossy_size: 500_000_000,
            optimal_bitrate: 256,
        };
        match event {
            EncoderEvent::PhaseTransition {
                phase,
                measured_lossy_size,
                optimal_bitrate,
            } => {
                assert_eq!(phase, EncodingPhase::LosslessPass);
                assert_eq!(measured_lossy_size, 500_000_000);
                assert_eq!(optimal_bitrate, 256);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_encoder_event_clone() {
        let event = EncoderEvent::FolderStarted {
            id: FolderId("clone_test".to_string()),
            files_total: 5,
        };
        let cloned = event.clone();
        match cloned {
            EncoderEvent::FolderStarted { id, files_total } => {
                assert_eq!(id.0, "clone_test");
                assert_eq!(files_total, 5);
            }
            _ => panic!("Wrong variant after clone"),
        }
    }

    #[test]
    fn test_encoder_event_debug() {
        let event = EncoderEvent::FolderCancelled(FolderId("debug_test".to_string()));
        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("FolderCancelled"));
        assert!(debug_str.contains("debug_test"));
    }
}
