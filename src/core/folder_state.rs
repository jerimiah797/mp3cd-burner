//! Folder state tracking for background encoding
//!
//! This module provides types for tracking the conversion state of each folder
//! in the list, enabling incremental encoding, smart reordering, and profile persistence.

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Unique identifier for a folder based on its path and modification time
///
/// This allows us to detect when source files have changed and need re-encoding.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FolderId(pub String);

impl FolderId {
    /// Create a FolderId from a path by hashing the path and its modification time
    pub fn from_path(path: &Path) -> Self {
        let mtime = fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs())
            .unwrap_or(0);

        let mut hasher = DefaultHasher::new();
        path.to_string_lossy().hash(&mut hasher);
        mtime.hash(&mut hasher);

        FolderId(format!("{:016x}", hasher.finish()))
    }

    /// Extract the modification time used to create this ID (for comparison)
    /// Note: This is a simplified version - in practice we'd store mtime separately
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for FolderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Reason why a folder needs to be re-encoded
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ReencodeReason {
    /// Lossless bitrate changed due to capacity constraints
    BitrateChanged { old: u32, new: u32 },
    /// Source files were modified since last encoding
    SourceFilesModified,
    /// ISO was too large, need to reduce quality
    IsoSizeExceeded,
}

/// Per-folder conversion state
///
/// Tracks the encoding status of each folder in the list, enabling:
/// - Background encoding progress tracking
/// - Smart detection of what needs re-encoding
/// - Profile persistence and restoration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FolderConversionStatus {
    /// Not yet converted - queued or waiting
    NotConverted,

    /// Currently being converted
    Converting {
        /// Number of files completed
        files_completed: usize,
        /// Total number of files
        files_total: usize,
    },

    /// Successfully converted
    Converted {
        /// Output directory path (e.g., /tmp/mp3cd_output/{session}/{folder_id}/)
        output_dir: PathBuf,
        /// Bitrate used for lossless files in this folder (None if no lossless)
        lossless_bitrate: Option<u32>,
        /// Total output size in bytes
        output_size: u64,
        /// When conversion completed (Unix timestamp)
        completed_at: u64,
    },

    /// Needs re-encoding due to changed conditions
    NeedsReencode {
        /// Previous output directory (may be cleaned up)
        previous_output_dir: Option<PathBuf>,
        /// Why re-encoding is needed
        reason: ReencodeReason,
    },
}

impl Default for FolderConversionStatus {
    fn default() -> Self {
        Self::NotConverted
    }
}

/// Calculate a hash representing the current folder list order
///
/// Used to detect if the ISO needs regeneration after reordering.
pub fn calculate_folder_hash(folder_ids: &[FolderId]) -> String {
    let mut hasher = DefaultHasher::new();
    for id in folder_ids {
        id.0.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_folder_id_from_path() {
        let temp_dir = TempDir::new().unwrap();
        let id1 = FolderId::from_path(temp_dir.path());
        let id2 = FolderId::from_path(temp_dir.path());
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_folder_id_different_paths() {
        let temp_dir1 = TempDir::new().unwrap();
        let temp_dir2 = TempDir::new().unwrap();
        let id1 = FolderId::from_path(temp_dir1.path());
        let id2 = FolderId::from_path(temp_dir2.path());
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_folder_conversion_status_default() {
        let status = FolderConversionStatus::default();
        assert!(matches!(status, FolderConversionStatus::NotConverted));
    }

    #[test]
    fn test_folder_conversion_status_converted() {
        let status = FolderConversionStatus::Converted {
            output_dir: PathBuf::from("/tmp/test"),
            lossless_bitrate: Some(320),
            output_size: 1000,
            completed_at: 0,
        };
        assert!(matches!(status, FolderConversionStatus::Converted { .. }));
    }

    #[test]
    fn test_calculate_folder_hash() {
        let ids1 = vec![FolderId("abc".to_string()), FolderId("def".to_string())];
        let ids2 = vec![FolderId("def".to_string()), FolderId("abc".to_string())];
        let ids3 = vec![FolderId("abc".to_string()), FolderId("def".to_string())];

        let hash1 = calculate_folder_hash(&ids1);
        let hash2 = calculate_folder_hash(&ids2);
        let hash3 = calculate_folder_hash(&ids3);

        assert_ne!(hash1, hash2); // Different order = different hash
        assert_eq!(hash1, hash3); // Same order = same hash
    }
}
