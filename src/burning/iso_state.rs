//! ISO state tracking for "Burn Another" functionality
//!
//! This module tracks the state of the created ISO image, enabling:
//! - Re-burning the same ISO without re-converting
//! - Detecting when ISO needs regeneration (folder order changed)
//! - Detecting when folders need re-encoding (content changed)

use std::path::{Path, PathBuf};

use crate::core::{calculate_folder_hash, FolderId, MusicFolder};

/// Maximum ISO size for CD burning (699 MB)
pub const MAX_ISO_SIZE_BYTES: u64 = 699 * 1024 * 1024;

/// Tracks the state of the current ISO image
#[derive(Debug, Clone)]
pub struct IsoState {
    /// Path to the ISO file
    pub path: PathBuf,
    /// Hash of folder IDs + order used to create this ISO
    pub folder_hash: String,
    /// Size of the ISO in bytes
    pub size_bytes: u64,
    /// Whether the ISO is valid (exists and matches current folders)
    pub is_valid: bool,
}

impl IsoState {
    /// Create a new IsoState after ISO creation
    pub fn new(path: PathBuf, folders: &[MusicFolder]) -> Result<Self, String> {
        let folder_ids: Vec<FolderId> = folders.iter().map(|f| f.id.clone()).collect();
        let folder_hash = calculate_folder_hash(&folder_ids);

        let size_bytes = std::fs::metadata(&path)
            .map(|m| m.len())
            .map_err(|e| format!("Failed to get ISO size: {}", e))?;

        Ok(Self {
            path,
            folder_hash,
            size_bytes,
            is_valid: true,
        })
    }

    /// Check if the ISO matches the current folder list
    pub fn matches_folders(&self, folders: &[MusicFolder]) -> bool {
        let folder_ids: Vec<FolderId> = folders.iter().map(|f| f.id.clone()).collect();
        let current_hash = calculate_folder_hash(&folder_ids);
        self.folder_hash == current_hash
    }

    /// Check if the ISO file still exists
    pub fn file_exists(&self) -> bool {
        self.path.exists()
    }

    /// Check if the ISO is ready for burning
    ///
    /// Returns true if the ISO exists, is valid, and matches the current folders.
    pub fn is_ready_to_burn(&self, folders: &[MusicFolder]) -> bool {
        self.is_valid && self.file_exists() && self.matches_folders(folders)
    }

    /// Check if the ISO size exceeds the CD limit
    pub fn exceeds_cd_limit(&self) -> bool {
        self.size_bytes > MAX_ISO_SIZE_BYTES
    }

    /// Invalidate the ISO (e.g., when folders change)
    pub fn invalidate(&mut self) {
        self.is_valid = false;
    }

    /// Get the ISO path
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Determines what action is needed when folders change
#[derive(Debug, Clone, PartialEq)]
pub enum IsoAction {
    /// ISO is still valid, can burn directly
    BurnExisting,
    /// Need to regenerate ISO (folder order changed, but same content)
    RegenerateIso,
    /// Need to encode some folders, then regenerate ISO
    EncodeAndRegenerate {
        /// Folder IDs that need encoding
        folders_to_encode: Vec<FolderId>,
    },
    /// No ISO exists yet, need full conversion
    FullConversion,
}

/// Determine what action is needed based on current state
pub fn determine_iso_action(
    iso_state: Option<&IsoState>,
    current_folders: &[MusicFolder],
    encoded_folder_ids: &[FolderId],
) -> IsoAction {
    // No folders = nothing to do
    if current_folders.is_empty() {
        return IsoAction::FullConversion;
    }

    // Check if we have a valid ISO
    match iso_state {
        Some(iso) if iso.is_valid && iso.file_exists() => {
            if iso.matches_folders(current_folders) {
                // ISO matches current folders exactly - can burn directly
                IsoAction::BurnExisting
            } else {
                // ISO doesn't match - check if it's just a reorder
                let current_ids: Vec<FolderId> =
                    current_folders.iter().map(|f| f.id.clone()).collect();

                // Check if all current folders are already encoded
                let all_encoded = current_ids
                    .iter()
                    .all(|id| encoded_folder_ids.contains(id));

                if all_encoded {
                    // All folders encoded, just need to regenerate ISO with new order
                    IsoAction::RegenerateIso
                } else {
                    // Some folders need encoding
                    let folders_to_encode: Vec<FolderId> = current_ids
                        .into_iter()
                        .filter(|id| !encoded_folder_ids.contains(id))
                        .collect();

                    IsoAction::EncodeAndRegenerate { folders_to_encode }
                }
            }
        }
        _ => {
            // No valid ISO - check if we need encoding
            let current_ids: Vec<FolderId> =
                current_folders.iter().map(|f| f.id.clone()).collect();

            let all_encoded = current_ids
                .iter()
                .all(|id| encoded_folder_ids.contains(id));

            if all_encoded {
                // All folders encoded, just need to create ISO
                IsoAction::RegenerateIso
            } else if encoded_folder_ids.is_empty() {
                // Nothing encoded yet
                IsoAction::FullConversion
            } else {
                // Some folders need encoding
                let folders_to_encode: Vec<FolderId> = current_ids
                    .into_iter()
                    .filter(|id| !encoded_folder_ids.contains(id))
                    .collect();

                IsoAction::EncodeAndRegenerate { folders_to_encode }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_iso_state_matches_folders() {
        let folders = vec![
            MusicFolder::new_for_test_with_id("album1"),
            MusicFolder::new_for_test_with_id("album2"),
        ];

        let folder_ids: Vec<FolderId> = folders.iter().map(|f| f.id.clone()).collect();
        let hash = calculate_folder_hash(&folder_ids);

        let iso = IsoState {
            path: PathBuf::from("/tmp/test.iso"),
            folder_hash: hash,
            size_bytes: 100_000_000,
            is_valid: true,
        };

        assert!(iso.matches_folders(&folders));

        // Different order should not match
        let reordered = vec![
            MusicFolder::new_for_test_with_id("album2"),
            MusicFolder::new_for_test_with_id("album1"),
        ];
        assert!(!iso.matches_folders(&reordered));
    }

    #[test]
    fn test_iso_state_exceeds_limit() {
        let iso_under = IsoState {
            path: PathBuf::from("/tmp/test.iso"),
            folder_hash: "abc".to_string(),
            size_bytes: 600 * 1024 * 1024, // 600 MB
            is_valid: true,
        };
        assert!(!iso_under.exceeds_cd_limit());

        let iso_over = IsoState {
            path: PathBuf::from("/tmp/test.iso"),
            folder_hash: "abc".to_string(),
            size_bytes: 750 * 1024 * 1024, // 750 MB
            is_valid: true,
        };
        assert!(iso_over.exceeds_cd_limit());
    }

    #[test]
    fn test_determine_iso_action_no_folders() {
        let action = determine_iso_action(None, &[], &[]);
        assert_eq!(action, IsoAction::FullConversion);
    }

    #[test]
    fn test_determine_iso_action_no_iso() {
        let folders = vec![MusicFolder::new_for_test_with_id("album1")];
        let action = determine_iso_action(None, &folders, &[]);
        assert_eq!(action, IsoAction::FullConversion);
    }

    #[test]
    fn test_determine_iso_action_all_encoded_no_iso() {
        let folders = vec![MusicFolder::new_for_test_with_id("album1")];
        let encoded = vec![FolderId("album1".to_string())];
        let action = determine_iso_action(None, &folders, &encoded);
        assert_eq!(action, IsoAction::RegenerateIso);
    }

    #[test]
    fn test_determine_iso_action_partial_encoded() {
        let folders = vec![
            MusicFolder::new_for_test_with_id("album1"),
            MusicFolder::new_for_test_with_id("album2"),
        ];
        let encoded = vec![FolderId("album1".to_string())];
        let action = determine_iso_action(None, &folders, &encoded);

        match action {
            IsoAction::EncodeAndRegenerate { folders_to_encode } => {
                assert_eq!(folders_to_encode.len(), 1);
                assert_eq!(folders_to_encode[0].0, "album2");
            }
            _ => panic!("Expected EncodeAndRegenerate"),
        }
    }

    #[test]
    fn test_determine_iso_action_valid_iso_matches() {
        let folders = vec![MusicFolder::new_for_test_with_id("album1")];
        let folder_ids: Vec<FolderId> = folders.iter().map(|f| f.id.clone()).collect();
        let hash = calculate_folder_hash(&folder_ids);

        // Create a temp file to simulate existing ISO
        let temp_dir = tempfile::tempdir().unwrap();
        let iso_path = temp_dir.path().join("test.iso");
        std::fs::write(&iso_path, "fake iso content").unwrap();

        let iso = IsoState {
            path: iso_path,
            folder_hash: hash,
            size_bytes: 100_000_000,
            is_valid: true,
        };

        let encoded = vec![FolderId("album1".to_string())];
        let action = determine_iso_action(Some(&iso), &folders, &encoded);
        assert_eq!(action, IsoAction::BurnExisting);
    }

    #[test]
    fn test_determine_iso_action_reorder_only() {
        let folders = vec![
            MusicFolder::new_for_test_with_id("album2"), // Reordered
            MusicFolder::new_for_test_with_id("album1"),
        ];

        // Original order hash
        let original_folders = vec![
            MusicFolder::new_for_test_with_id("album1"),
            MusicFolder::new_for_test_with_id("album2"),
        ];
        let original_ids: Vec<FolderId> = original_folders.iter().map(|f| f.id.clone()).collect();
        let original_hash = calculate_folder_hash(&original_ids);

        let temp_dir = tempfile::tempdir().unwrap();
        let iso_path = temp_dir.path().join("test.iso");
        std::fs::write(&iso_path, "fake iso content").unwrap();

        let iso = IsoState {
            path: iso_path,
            folder_hash: original_hash,
            size_bytes: 100_000_000,
            is_valid: true,
        };

        // Both folders are encoded
        let encoded = vec![
            FolderId("album1".to_string()),
            FolderId("album2".to_string()),
        ];

        let action = determine_iso_action(Some(&iso), &folders, &encoded);
        assert_eq!(action, IsoAction::RegenerateIso);
    }
}
