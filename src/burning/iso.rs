//! ISO creation using hdiutil
//! (Future: Stage 9)
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of ISO creation
#[derive(Debug)]
pub struct IsoResult {
    pub iso_path: PathBuf,
}

/// Create an ISO image from a directory using hdiutil
///
/// # Arguments
/// * `source_dir` - Directory containing files to burn
/// * `volume_label` - Label for the ISO volume
///
/// # Returns
/// * `Ok(IsoResult)` with the path to the created ISO
/// * `Err(String)` with error message on failure
pub fn create_iso(source_dir: &Path, volume_label: &str) -> Result<IsoResult, String> {
    let iso_path = source_dir
        .parent()
        .unwrap_or(source_dir)
        .join("mp3cd.iso");

    // Remove existing ISO file if it exists
    if iso_path.exists() {
        println!("Removing existing ISO file at {}", iso_path.display());
        fs::remove_file(&iso_path)
            .map_err(|e| format!("Failed to remove existing ISO: {}", e))?;
    }

    println!(
        "Creating ISO from {} to {} with volume label '{}'",
        source_dir.display(),
        iso_path.display(),
        volume_label
    );

    // Create ISO using hdiutil makehybrid with Joliet extensions and custom volume label
    let output = Command::new("hdiutil")
        .args([
            "makehybrid",
            "-iso",
            "-joliet",
            "-joliet-volume-name",
            volume_label,
            "-o",
            iso_path.to_str().unwrap(),
            source_dir.to_str().unwrap(),
        ])
        .output()
        .map_err(|e| format!("Failed to execute hdiutil makehybrid: {}", e))?;

    if output.status.success() {
        println!("ISO created successfully at {}", iso_path.display());
        Ok(IsoResult { iso_path })
    } else {
        let error_msg = String::from_utf8_lossy(&output.stderr);
        Err(format!("Failed to create ISO: {}", error_msg))
    }
}

/// Recursively copy a directory
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            fs::copy(&path, &dest_path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_copy_dir_recursive() {
        let temp_src = TempDir::new().unwrap();
        let temp_dst = TempDir::new().unwrap();

        let src_path = temp_src.path();
        let dst_path = temp_dst.path().join("copied");

        // Create test structure
        fs::write(src_path.join("file1.txt"), b"content1").unwrap();
        fs::create_dir(src_path.join("subdir")).unwrap();
        fs::write(src_path.join("subdir/file2.txt"), b"content2").unwrap();

        // Copy
        copy_dir_recursive(src_path, &dst_path).unwrap();

        // Verify
        assert!(dst_path.join("file1.txt").exists());
        assert!(dst_path.join("subdir").exists());
        assert!(dst_path.join("subdir/file2.txt").exists());

        let content1 = fs::read_to_string(dst_path.join("file1.txt")).unwrap();
        assert_eq!(content1, "content1");
    }
}
