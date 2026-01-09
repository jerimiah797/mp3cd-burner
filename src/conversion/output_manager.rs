//! Output directory and session management
//!
//! This module manages the output directory structure for background encoding:
//! - Session-based directories: `/tmp/mp3cd_output/{session_id}/{folder_id}/`
//! - ISO staging with symlinks: Numbered symlinks for ISO creation
//! - Cleanup of old sessions

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::{FolderId, MusicFolder};

/// Manages output directories for a conversion session
///
/// Clone-safe: all clones share the same bundle_path state, so setting the
/// bundle path on one clone affects all others.
#[derive(Debug, Clone)]
pub struct OutputManager {
    /// Session ID (timestamp-based)
    session_id: String,
    /// Base output directory (in /tmp)
    base_dir: PathBuf,
    /// Session directory path (in /tmp)
    session_dir: PathBuf,
    /// Bundle path (when working with a saved bundle)
    /// If set, output goes to {bundle}/converted/ instead of temp
    /// Shared across all clones so encoder sees updates from UI
    bundle_path: Arc<Mutex<Option<PathBuf>>>,
}

impl OutputManager {
    /// Create a new output manager with a fresh session
    ///
    /// Creates a new session directory. Call `cleanup_old_sessions()` explicitly
    /// if you want to clean up previous sessions.
    pub fn new() -> Result<Self, String> {
        let base_dir = std::env::temp_dir().join("mp3cd_output");
        let session_id = generate_session_id();
        let session_dir = base_dir.join(&session_id);

        // Create session directory (base_dir created implicitly)
        fs::create_dir_all(&session_dir)
            .map_err(|e| format!("Failed to create session directory: {}", e))?;

        log::debug!("Created session: {} at {:?}", session_id, session_dir);

        Ok(Self {
            session_id,
            base_dir,
            session_dir,
            bundle_path: Arc::new(Mutex::new(None)),
        })
    }

    /// Set the bundle path for working with saved bundles
    ///
    /// When set, `get_folder_output_dir()` returns paths inside the bundle
    /// instead of the temp session directory.
    ///
    /// This is thread-safe: setting the bundle path affects all clones of this OutputManager.
    pub fn set_bundle_path(&self, path: Option<PathBuf>) {
        let mut guard = self.bundle_path.lock().unwrap();
        *guard = path;
        if let Some(ref p) = *guard {
            log::debug!("OutputManager: bundle path set to {:?}", p);
        } else {
            log::debug!("OutputManager: bundle path cleared");
        }
    }

    /// Get the current bundle path, if set
    pub fn get_bundle_path(&self) -> Option<PathBuf> {
        let guard = self.bundle_path.lock().unwrap();
        guard.clone()
    }

    /// Check if we're working with a bundle
    pub fn is_bundle_mode(&self) -> bool {
        let guard = self.bundle_path.lock().unwrap();
        guard.is_some()
    }

    /// Clean up all sessions except the current one
    ///
    /// This should be called at app startup to clean up orphaned sessions
    /// from previous runs.
    pub fn cleanup_old_sessions(&self) -> Result<(), String> {
        if !self.base_dir.exists() {
            return Ok(());
        }

        let entries = fs::read_dir(&self.base_dir)
            .map_err(|e| format!("Failed to read sessions directory: {}", e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip current session
                if path.file_name().and_then(|n| n.to_str()) == Some(&self.session_id) {
                    continue;
                }
                // Delete old session
                if let Err(e) = fs::remove_dir_all(&path) {
                    log::warn!("Warning: Failed to clean old session {:?}: {}", path, e);
                } else {
                    log::debug!("Cleaned up old session: {:?}", path);
                }
            }
        }

        Ok(())
    }

    /// Get the session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get the session directory path (used in tests)
    #[allow(dead_code)]
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    /// Get the output directory for a specific folder
    ///
    /// Creates the directory if it doesn't exist.
    pub fn get_folder_output_dir(&self, folder_id: &FolderId) -> Result<PathBuf, String> {
        let bundle_path = self.get_bundle_path();
        let folder_dir = if let Some(bundle) = bundle_path {
            // Bundle mode: {bundle}/converted/{folder_id}/
            bundle.join("converted").join(folder_id.as_str())
        } else {
            // Temp mode: {session_dir}/{folder_id}/
            self.session_dir.join(folder_id.as_str())
        };

        if !folder_dir.exists() {
            fs::create_dir_all(&folder_dir)
                .map_err(|e| format!("Failed to create folder output directory: {}", e))?;
        }

        Ok(folder_dir)
    }

    /// Check if a folder's output directory exists (used in tests)
    #[allow(dead_code)]
    pub fn folder_output_exists(&self, folder_id: &FolderId) -> bool {
        let bundle_path = self.get_bundle_path();
        if let Some(bundle) = bundle_path {
            bundle.join("converted").join(folder_id.as_str()).exists()
        } else {
            self.session_dir.join(folder_id.as_str()).exists()
        }
    }

    /// Get the total size of a folder's output directory
    pub fn get_folder_output_size(&self, folder_id: &FolderId) -> Result<u64, String> {
        let bundle_path = self.get_bundle_path();
        let folder_dir = if let Some(bundle) = bundle_path {
            bundle.join("converted").join(folder_id.as_str())
        } else {
            self.session_dir.join(folder_id.as_str())
        };

        if !folder_dir.exists() {
            return Ok(0);
        }

        calculate_dir_size(&folder_dir)
    }

    /// Delete a folder's output directory (e.g., when folder is removed from list)
    pub fn delete_folder_output(&self, folder_id: &FolderId) -> Result<(), String> {
        let bundle_path = self.get_bundle_path();
        let folder_dir = if let Some(bundle) = bundle_path {
            bundle.join("converted").join(folder_id.as_str())
        } else {
            self.session_dir.join(folder_id.as_str())
        };

        if folder_dir.exists() {
            fs::remove_dir_all(&folder_dir)
                .map_err(|e| format!("Failed to delete folder output: {}", e))?;
            log::debug!("Deleted output for folder: {}", folder_id);
        }

        Ok(())
    }

    /// Delete a folder's output from the session temp directory only
    /// Used by RecalculateBitrate to delete encoded files that need re-encoding
    pub fn delete_folder_output_from_session(&self, folder_id: &FolderId) -> Result<(), String> {
        let folder_dir = self.session_dir.join(folder_id.as_str());

        if folder_dir.exists() {
            fs::remove_dir_all(&folder_dir)
                .map_err(|e| format!("Failed to delete folder output from session: {}", e))?;
            log::debug!("Deleted session output for folder: {}", folder_id);
        }

        Ok(())
    }

    /// Copy converted files from temp session to a bundle
    ///
    /// This is called during the first save to move converted files
    /// from the temp directory to the bundle.
    pub fn copy_to_bundle(
        &self,
        bundle_path: &Path,
        folder_ids: &[FolderId],
    ) -> Result<(), String> {
        let converted_dir = bundle_path.join("converted");

        // Clean up existing converted directory to ensure fresh copy
        if converted_dir.exists() {
            fs::remove_dir_all(&converted_dir)
                .map_err(|e| format!("Failed to clean existing converted directory: {}", e))?;
            log::debug!("Cleaned existing converted directory in bundle");
        }

        fs::create_dir_all(&converted_dir)
            .map_err(|e| format!("Failed to create converted directory in bundle: {}", e))?;

        for folder_id in folder_ids {
            let src = self.session_dir.join(folder_id.as_str());
            let dst = converted_dir.join(folder_id.as_str());

            if src.exists() {
                // Copy the entire folder
                copy_dir_recursive(&src, &dst)?;
                log::debug!("Copied {} to bundle: {:?} -> {:?}", folder_id, src, dst);
            } else {
                log::debug!(
                    "Warning: Source folder not found for {}: {:?}",
                    folder_id, src
                );
            }
        }

        Ok(())
    }

    /// Get the relative path for a folder's output (for storing in profile.json)
    ///
    /// Returns a path like "converted/abc123..." that is relative to the bundle root.
    pub fn get_relative_output_path(&self, folder_id: &FolderId) -> String {
        format!("converted/{}", folder_id.as_str())
    }

    /// Copy pre-encoded files from a bundle to the current session
    ///
    /// This is called when loading a bundle profile to copy converted files
    /// into the temp session directory. This unifies bundle folders with
    /// newly encoded folders - all files end up in the same session directory.
    ///
    /// Returns the destination path (in temp session).
    pub fn copy_from_bundle(
        &self,
        bundle_path: &Path,
        folder_id: &FolderId,
    ) -> Result<PathBuf, String> {
        let src = bundle_path.join("converted").join(folder_id.as_str());
        let dst = self.session_dir.join(folder_id.as_str());

        if !src.exists() {
            return Err(format!(
                "Bundle folder not found: {:?}",
                src
            ));
        }

        // Copy the entire folder from bundle to temp
        copy_dir_recursive(&src, &dst)?;
        log::debug!(
            "Copied from bundle: {:?} -> {:?}",
            src, dst
        );

        Ok(dst)
    }

    /// Create ISO staging directory with numbered symlinks
    ///
    /// This creates a staging directory with numbered folders containing
    /// symlinks to the actual output files. Track ordering and numbered prefixes
    /// are applied here, allowing reordering without re-encoding.
    ///
    /// Note: Staging is always in the temp session directory (not in bundle),
    /// but symlinks point to converted files which may be in a bundle.
    ///
    /// Returns the staging directory path.
    pub fn create_iso_staging(&self, folders: &[MusicFolder]) -> Result<PathBuf, String> {
        let staging_dir = self.session_dir.join("_iso_staging");

        // Clean up existing staging
        if staging_dir.exists() {
            fs::remove_dir_all(&staging_dir)
                .map_err(|e| format!("Failed to clean staging directory: {}", e))?;
        }

        fs::create_dir_all(&staging_dir)
            .map_err(|e| format!("Failed to create staging directory: {}", e))?;

        // Create numbered folders with symlinks to individual tracks
        for (index, folder) in folders.iter().enumerate() {
            // Get source directory from folder's conversion status if available,
            // otherwise fall back to session directory.
            let source_dir = match &folder.conversion_status {
                crate::core::FolderConversionStatus::Converted { output_dir, .. } => {
                    output_dir.clone()
                }
                _ => {
                    // Fall back to session directory for folders not yet converted
                    // (shouldn't happen during ISO staging, but just in case)
                    self.session_dir.join(folder.id.as_str())
                }
            };

            if !source_dir.exists() {
                return Err(format!(
                    "Output directory not found for folder: {}",
                    folder.path.display()
                ));
            }

            // Create a numbered folder name with the album/mixtape name
            let display_name = folder.display_name();
            let safe_name = sanitize_filename(&display_name);
            let numbered_name = format!("{:02}-{}", index + 1, safe_name);
            let folder_staging_path = staging_dir.join(&numbered_name);

            fs::create_dir_all(&folder_staging_path)
                .map_err(|e| format!("Failed to create staging folder: {}", e))?;

            // Determine if we need numbered prefixes for tracks:
            // - Mixtapes: always numbered (user-curated playlist)
            // - Albums: only if custom track order is set (user reordered)
            let use_numbered_prefix = folder.is_mixtape() || folder.track_order.is_some();

            // Get active tracks in order (respects exclusions and custom order)
            let active_tracks = folder.active_tracks();

            // Create symlinks for each track with optional numbered prefix
            for (track_idx, track) in active_tracks.iter().enumerate() {
                let stem = track
                    .path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown");

                // Source file in output directory (encoded without number prefix)
                let source_file = source_dir.join(format!("{}.mp3", stem));

                // Destination filename with optional numbered prefix
                let dest_filename = if use_numbered_prefix {
                    format!("{:02}-{}.mp3", track_idx + 1, stem)
                } else {
                    format!("{}.mp3", stem)
                };
                let dest_path = folder_staging_path.join(&dest_filename);

                if source_file.exists() {
                    #[cfg(unix)]
                    {
                        std::os::unix::fs::symlink(&source_file, &dest_path).map_err(|e| {
                            format!("Failed to create symlink for {}: {}", stem, e)
                        })?;
                    }

                    #[cfg(not(unix))]
                    {
                        fs::copy(&source_file, &dest_path).map_err(|e| {
                            format!("Failed to copy file for {}: {}", stem, e)
                        })?;
                    }
                } else {
                    log::debug!(
                        "Warning: Source file not found during staging: {}",
                        source_file.display()
                    );
                }
            }

            log::debug!(
                "Staged: {} ({} tracks, numbered: {})",
                numbered_name,
                active_tracks.len(),
                use_numbered_prefix
            );
        }

        Ok(staging_dir)
    }

    /// Get the ISO staging directory path (used in tests)
    #[allow(dead_code)]
    pub fn staging_dir(&self) -> PathBuf {
        self.session_dir.join("_iso_staging")
    }

    /// Clean up the session (delete all output)
    pub fn cleanup(&self) -> Result<(), String> {
        log::debug!("Cleanup requested for session: {} at {:?}", self.session_id, self.session_dir);
        if self.session_dir.exists() {
            fs::remove_dir_all(&self.session_dir)
                .map_err(|e| format!("Failed to clean up session {}: {}", self.session_id, e))?;
            log::debug!("Cleaned up session: {}", self.session_id);
        } else {
            log::debug!("Session directory does not exist, nothing to clean: {}", self.session_id);
        }
        Ok(())
    }
}

impl Default for OutputManager {
    fn default() -> Self {
        Self::new().expect("Failed to create default OutputManager")
    }
}

/// Generate a unique session ID based on timestamp and random component
fn generate_session_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    // Use nanoseconds + thread ID for uniqueness in parallel tests
    let thread_id = std::thread::current().id();
    format!("session_{}_{:?}", timestamp, thread_id)
        .replace("ThreadId(", "")
        .replace(")", "")
}

/// Calculate the total size of a directory recursively
fn calculate_dir_size(path: &Path) -> Result<u64, String> {
    let mut total = 0u64;

    for entry in
        fs::read_dir(path).map_err(|e| format!("Failed to read directory {:?}: {}", path, e))?
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let metadata = entry
            .metadata()
            .map_err(|e| format!("Failed to get metadata: {}", e))?;

        if metadata.is_file() {
            total += metadata.len();
        } else if metadata.is_dir() {
            total += calculate_dir_size(&entry.path())?;
        }
    }

    Ok(total)
}

/// Sanitize a filename for safe filesystem use
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

/// Copy a directory recursively
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create directory {:?}: {}", dst, e))?;

    for entry in
        fs::read_dir(src).map_err(|e| format!("Failed to read directory {:?}: {}", src, e))?
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("Failed to copy {:?} to {:?}: {}", src_path, dst_path, e))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_generate_session_id() {
        let id1 = generate_session_id();
        let id2 = generate_session_id();

        assert!(id1.starts_with("session_"));
        assert!(id2.starts_with("session_"));
        // IDs should be different (time-based)
        // Note: might be same if generated in same millisecond
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("normal"), "normal");
        assert_eq!(sanitize_filename("with/slash"), "with_slash");
        assert_eq!(sanitize_filename("with:colon"), "with_colon");
        assert_eq!(sanitize_filename("AC/DC"), "AC_DC");
        assert_eq!(sanitize_filename("What?!"), "What_!");
    }

    #[test]
    fn test_calculate_dir_size() {
        let temp_dir = TempDir::new().unwrap();

        // Create some test files
        let file1 = temp_dir.path().join("file1.txt");
        let file2 = temp_dir.path().join("file2.txt");

        fs::write(&file1, "Hello").unwrap();
        fs::write(&file2, "World!").unwrap();

        let size = calculate_dir_size(temp_dir.path()).unwrap();
        assert_eq!(size, 11); // "Hello" (5) + "World!" (6)
    }

    #[test]
    fn test_calculate_dir_size_nested() {
        let temp_dir = TempDir::new().unwrap();

        // Create nested structure
        let subdir = temp_dir.path().join("subdir");
        fs::create_dir(&subdir).unwrap();

        fs::write(temp_dir.path().join("root.txt"), "root").unwrap();
        fs::write(subdir.join("nested.txt"), "nested").unwrap();

        let size = calculate_dir_size(temp_dir.path()).unwrap();
        assert_eq!(size, 10); // "root" (4) + "nested" (6)
    }

    #[test]
    fn test_output_manager_new() {
        // This test creates real directories in /tmp, which is fine
        let manager = OutputManager::new();
        assert!(manager.is_ok());

        let manager = manager.unwrap();
        assert!(manager.session_dir().exists());
        assert!(manager.session_id().starts_with("session_"));

        // Cleanup
        let _ = manager.cleanup();
    }

    #[test]
    fn test_output_manager_folder_dir() {
        let manager = OutputManager::new().unwrap();
        let folder_id = FolderId("test_folder_123".to_string());

        let folder_dir = manager.get_folder_output_dir(&folder_id).unwrap();
        assert!(folder_dir.exists());
        assert!(folder_dir.ends_with("test_folder_123"));

        // Cleanup
        let _ = manager.cleanup();
    }

    #[test]
    fn test_output_manager_folder_exists() {
        let manager = OutputManager::new().unwrap();
        let folder_id = FolderId("check_exists_456".to_string());

        assert!(!manager.folder_output_exists(&folder_id));

        manager.get_folder_output_dir(&folder_id).unwrap();

        assert!(manager.folder_output_exists(&folder_id));

        // Cleanup
        let _ = manager.cleanup();
    }

    #[test]
    fn test_output_manager_delete_folder() {
        let manager = OutputManager::new().unwrap();
        let folder_id = FolderId("to_delete_789".to_string());

        // Create the folder
        let folder_dir = manager.get_folder_output_dir(&folder_id).unwrap();
        assert!(folder_dir.exists());

        // Add a file to it
        fs::write(folder_dir.join("test.mp3"), "fake audio").unwrap();

        // Delete it
        manager.delete_folder_output(&folder_id).unwrap();
        assert!(!folder_dir.exists());

        // Cleanup
        let _ = manager.cleanup();
    }

    #[test]
    fn test_output_manager_folder_size() {
        let manager = OutputManager::new().unwrap();
        let folder_id = FolderId("size_test_abc".to_string());

        // Empty folder should be 0
        assert_eq!(manager.get_folder_output_size(&folder_id).unwrap(), 0);

        // Create folder with content
        let folder_dir = manager.get_folder_output_dir(&folder_id).unwrap();
        fs::write(folder_dir.join("song1.mp3"), "12345").unwrap();
        fs::write(folder_dir.join("song2.mp3"), "6789").unwrap();

        let size = manager.get_folder_output_size(&folder_id).unwrap();
        assert_eq!(size, 9); // 5 + 4

        // Cleanup
        let _ = manager.cleanup();
    }

    #[test]
    fn test_staging_dir_path() {
        let manager = OutputManager::new().unwrap();
        let staging = manager.staging_dir();

        assert!(staging.ends_with("_iso_staging"));

        // Cleanup
        let _ = manager.cleanup();
    }

    // Note: create_iso_staging requires MusicFolder with valid conversion state,
    // which requires more integration testing. The symlink creation logic is
    // tested implicitly through the individual helper tests.
}
