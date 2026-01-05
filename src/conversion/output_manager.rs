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

        println!("Created session: {} at {:?}", session_id, session_dir);

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
            println!("OutputManager: bundle path set to {:?}", p);
        } else {
            println!("OutputManager: bundle path cleared");
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
                    eprintln!("Warning: Failed to clean old session {:?}: {}", path, e);
                } else {
                    println!("Cleaned up old session: {:?}", path);
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
            println!("Deleted output for folder: {}", folder_id);
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
        fs::create_dir_all(&converted_dir)
            .map_err(|e| format!("Failed to create converted directory in bundle: {}", e))?;

        for folder_id in folder_ids {
            let src = self.session_dir.join(folder_id.as_str());
            let dst = converted_dir.join(folder_id.as_str());

            if src.exists() {
                // Copy the entire folder
                copy_dir_recursive(&src, &dst)?;
                println!("Copied {} to bundle: {:?} -> {:?}", folder_id, src, dst);
            } else {
                println!(
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

    /// Create ISO staging directory with numbered symlinks
    ///
    /// This creates a staging directory with numbered folders that symlink
    /// to the actual output folders. This allows reordering without re-encoding.
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

        // Create numbered symlinks to each folder's output
        for (index, folder) in folders.iter().enumerate() {
            // Get source directory from folder's conversion status if available,
            // otherwise fall back to session directory.
            // This handles folders loaded from bundles (which have output_dir set)
            // as well as newly encoded folders (which are in the session directory).
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

            // Create a numbered folder name with the album name
            let album_name = folder
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown");

            // Sanitize album name for filesystem
            let safe_name = sanitize_filename(album_name);
            let numbered_name = format!("{:02}-{}", index + 1, safe_name);
            let symlink_path = staging_dir.join(&numbered_name);

            // Create symlink
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&source_dir, &symlink_path).map_err(|e| {
                    format!(
                        "Failed to create symlink for {}: {}",
                        folder.path.display(),
                        e
                    )
                })?;
            }

            #[cfg(not(unix))]
            {
                // On non-Unix, copy the directory instead
                copy_dir_recursive(&source_dir, &symlink_path)?;
            }

            println!("Staged: {} -> {:?}", numbered_name, source_dir);
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
        if self.session_dir.exists() {
            fs::remove_dir_all(&self.session_dir)
                .map_err(|e| format!("Failed to clean up session: {}", e))?;
            println!("Cleaned up session: {}", self.session_id);
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
