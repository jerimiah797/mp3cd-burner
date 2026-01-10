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

    /// Create a new FolderId for a mixtape using a UUID
    pub fn new_mixtape() -> Self {
        FolderId(format!("mixtape:{}", uuid::Uuid::new_v4()))
    }

    /// Check if this FolderId represents a mixtape
    pub fn is_mixtape(&self) -> bool {
        self.0.starts_with("mixtape:")
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

    #[test]
    fn test_folder_id_new_mixtape() {
        let id = FolderId::new_mixtape();
        assert!(id.0.starts_with("mixtape:"));
        assert!(id.is_mixtape());
    }

    #[test]
    fn test_folder_id_is_mixtape() {
        let regular_id = FolderId("abc123".to_string());
        assert!(!regular_id.is_mixtape());

        let mixtape_id = FolderId("mixtape:some-uuid".to_string());
        assert!(mixtape_id.is_mixtape());
    }

    #[test]
    fn test_folder_id_as_str() {
        let id = FolderId("test_id_string".to_string());
        assert_eq!(id.as_str(), "test_id_string");
    }

    #[test]
    fn test_folder_id_display() {
        let id = FolderId("display_test".to_string());
        let display = format!("{}", id);
        assert_eq!(display, "display_test");
    }

    #[test]
    fn test_folder_conversion_status_converting() {
        let status = FolderConversionStatus::Converting {
            files_completed: 5,
            files_total: 10,
        };
        match status {
            FolderConversionStatus::Converting {
                files_completed,
                files_total,
            } => {
                assert_eq!(files_completed, 5);
                assert_eq!(files_total, 10);
            }
            _ => panic!("Expected Converting"),
        }
    }

    #[test]
    fn test_folder_conversion_status_needs_reencode() {
        let status = FolderConversionStatus::NeedsReencode {
            previous_output_dir: Some(PathBuf::from("/tmp/old")),
            reason: ReencodeReason::BitrateChanged { old: 320, new: 192 },
        };
        match status {
            FolderConversionStatus::NeedsReencode { reason, .. } => {
                assert!(matches!(reason, ReencodeReason::BitrateChanged { old: 320, new: 192 }));
            }
            _ => panic!("Expected NeedsReencode"),
        }
    }

    #[test]
    fn test_reencode_reason_variants() {
        let bitrate_changed = ReencodeReason::BitrateChanged { old: 256, new: 192 };
        let modified = ReencodeReason::SourceFilesModified;
        let size_exceeded = ReencodeReason::IsoSizeExceeded;

        assert!(matches!(bitrate_changed, ReencodeReason::BitrateChanged { .. }));
        assert!(matches!(modified, ReencodeReason::SourceFilesModified));
        assert!(matches!(size_exceeded, ReencodeReason::IsoSizeExceeded));
    }

    #[test]
    fn test_folder_id_hash() {
        use std::collections::HashSet;

        let id1 = FolderId("test1".to_string());
        let id2 = FolderId("test1".to_string());
        let id3 = FolderId("test2".to_string());

        let mut set = HashSet::new();
        set.insert(id1.clone());
        set.insert(id2.clone());
        set.insert(id3.clone());

        // id1 and id2 are equal, so set should have 2 elements
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_folder_id_from_nonexistent_path() {
        // Should still work (uses 0 for mtime)
        let id = FolderId::from_path(Path::new("/nonexistent/path/12345"));
        assert!(!id.0.is_empty());
    }

    #[test]
    fn test_calculate_folder_hash_empty() {
        let empty: Vec<FolderId> = vec![];
        let hash = calculate_folder_hash(&empty);
        assert!(!hash.is_empty());
    }

    #[test]
    fn test_calculate_folder_hash_single() {
        let single = vec![FolderId("single".to_string())];
        let hash = calculate_folder_hash(&single);
        assert_eq!(hash.len(), 16); // 64-bit hash as hex
    }
}
