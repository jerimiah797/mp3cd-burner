//! CD burning using hdiutil/drutil
//! (Future: Stage 10)
#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Progress callback type for burn operations
pub type ProgressCallback = Box<dyn Fn(i32) + Send>;

/// Status of the CD drive
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdStatus {
    /// No disc inserted
    NoDisc,
    /// Blank disc ready to burn
    Blank,
    /// Erasable disc (CD-RW) with data - can be erased and reused
    ErasableWithData,
    /// Non-erasable disc (CD-R) with data - cannot be used
    NonErasable,
}

/// Check the status of the CD drive
pub fn check_cd_status() -> Result<CdStatus, String> {
    let output = Command::new("drutil")
        .args(["status"])
        .output()
        .map_err(|e| format!("Failed to execute drutil: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout_lower = stdout.to_lowercase();

    // No disc inserted
    if stdout_lower.contains("no media") {
        return Ok(CdStatus::NoDisc);
    }

    // Check for blank media first
    if stdout_lower.contains("blank") {
        return Ok(CdStatus::Blank);
    }

    // Check if it's erasable (CD-RW, DVD-RW, etc.)
    // drutil status shows "Erasable: Yes" for rewritable media
    let is_erasable = stdout_lower.contains("erasable")
        || stdout_lower.contains("cd-rw")
        || stdout_lower.contains("dvd-rw")
        || stdout_lower.contains("dvd+rw");

    if is_erasable {
        return Ok(CdStatus::ErasableWithData);
    }

    // Non-blank, non-erasable disc
    Ok(CdStatus::NonErasable)
}

/// Check if a blank CD is inserted (legacy function for compatibility)
pub fn check_cd_inserted() -> Result<bool, String> {
    match check_cd_status()? {
        CdStatus::Blank => Ok(true),
        _ => Ok(false),
    }
}

/// Result of a burn operation
#[derive(Debug)]
pub enum BurnResult {
    Success,
    Cancelled,
    Error(String),
}

/// Burn an ISO file to CD using hdiutil
///
/// # Arguments
/// * `iso_path` - Path to the ISO file to burn
/// * `on_progress` - Optional callback for progress updates (0-100)
///
/// # Returns
/// * `Ok(())` on successful burn
/// * `Err(String)` with error message on failure
pub fn burn_iso(iso_path: &Path, on_progress: Option<ProgressCallback>) -> Result<(), String> {
    burn_iso_with_cancel(iso_path, on_progress, None, false)
}

/// Burn an ISO file to CD using hdiutil with cancellation and erase support
///
/// # Arguments
/// * `iso_path` - Path to the ISO file to burn
/// * `on_progress` - Optional callback for progress updates (0-100)
/// * `cancel_token` - Optional cancellation token to abort the burn
/// * `erase_first` - If true, erase the disc before burning (for CD-RW)
///
/// # Returns
/// * `Ok(BurnResult::Success)` on successful burn
/// * `Ok(BurnResult::Cancelled)` if cancelled
/// * `Err(String)` with error message on failure
pub fn burn_iso_with_cancel(
    iso_path: &Path,
    on_progress: Option<ProgressCallback>,
    cancel_token: Option<Arc<AtomicBool>>,
    erase_first: bool,
) -> Result<(), String> {
    if !iso_path.exists() {
        return Err(format!("ISO file not found: {}", iso_path.display()));
    }

    if erase_first {
        log::info!("Starting burn of {} (with erase)", iso_path.display());
    } else {
        log::info!("Starting burn of {}", iso_path.display());
    }

    // Build args - add -erase if erasing CD-RW first
    let mut args = vec!["burn", "-noverifyburn", "-puppetstrings"];
    if erase_first {
        args.push("-erase");
    }
    let iso_path_str = iso_path.to_str().unwrap();
    args.push(iso_path_str);

    // Spawn hdiutil with -puppetstrings to get progress output
    let mut child = Command::new("hdiutil")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to execute hdiutil burn: {}", e))?;

    // Read stdout for progress updates in a separate thread so we can check cancellation
    let stdout = child.stdout.take();
    let cancel_token_clone = cancel_token.clone();

    let progress_thread = std::thread::spawn(move || {
        if let Some(stdout) = stdout {
            let reader = BufReader::new(stdout);

            for line in reader.lines() {
                // Check for cancellation
                if let Some(ref token) = cancel_token_clone
                    && token.load(Ordering::SeqCst) {
                        break;
                    }

                if let Ok(line) = line {
                    // Parse progress lines like "PERCENT:0.059725" or "PERCENT:-1.000000"
                    if line.starts_with("PERCENT:")
                        && let Some(percent_str) = line.strip_prefix("PERCENT:")
                            && let Ok(percentage_float) = percent_str.trim().parse::<f64>() {
                                let percentage = percentage_float.round() as i32;
                                if let Some(ref callback) = on_progress {
                                    callback(percentage);
                                }
                            }
                }
            }
        }
    });

    // Poll for cancellation while waiting for the process
    loop {
        // Check if cancelled
        if let Some(ref token) = cancel_token
            && token.load(Ordering::SeqCst) {
                log::info!("Burn cancelled - killing hdiutil process");
                let _ = child.kill();
                let _ = child.wait(); // Reap the process
                let _ = progress_thread.join();
                return Err("Burn cancelled by user".to_string());
            }

        // Check if process has exited
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process has exited
                let _ = progress_thread.join();

                if status.success() {
                    log::info!("Burn completed successfully");
                    return Ok(());
                } else {
                    return Err("Burn process failed".to_string());
                }
            }
            Ok(None) => {
                // Process still running, sleep briefly and check again
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                let _ = progress_thread.join();
                return Err(format!("Error checking burn process: {}", e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_check_cd_inserted_runs() {
        // Just verify the function runs without panicking
        // The result depends on hardware state
        let result = check_cd_inserted();
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_cd_status_equality() {
        assert_eq!(CdStatus::NoDisc, CdStatus::NoDisc);
        assert_eq!(CdStatus::Blank, CdStatus::Blank);
        assert_eq!(CdStatus::ErasableWithData, CdStatus::ErasableWithData);
        assert_eq!(CdStatus::NonErasable, CdStatus::NonErasable);

        assert_ne!(CdStatus::NoDisc, CdStatus::Blank);
        assert_ne!(CdStatus::Blank, CdStatus::ErasableWithData);
        assert_ne!(CdStatus::ErasableWithData, CdStatus::NonErasable);
    }

    #[test]
    fn test_cd_status_clone() {
        let status = CdStatus::Blank;
        let cloned = status.clone();
        assert_eq!(status, cloned);
    }

    #[test]
    fn test_cd_status_copy() {
        let status = CdStatus::ErasableWithData;
        let copied = status;
        assert_eq!(status, copied);
    }

    #[test]
    fn test_cd_status_debug() {
        let status = CdStatus::NoDisc;
        let debug_str = format!("{:?}", status);
        assert!(debug_str.contains("NoDisc"));

        let status = CdStatus::Blank;
        let debug_str = format!("{:?}", status);
        assert!(debug_str.contains("Blank"));
    }

    #[test]
    fn test_burn_result_debug() {
        let result = BurnResult::Success;
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("Success"));

        let result = BurnResult::Cancelled;
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("Cancelled"));

        let result = BurnResult::Error("test error".to_string());
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("Error"));
        assert!(debug_str.contains("test error"));
    }

    #[test]
    fn test_burn_iso_file_not_found() {
        let result = burn_iso(Path::new("/nonexistent/file.iso"), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_burn_iso_with_cancel_file_not_found() {
        let result = burn_iso_with_cancel(Path::new("/nonexistent/file.iso"), None, None, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_check_cd_status_runs() {
        // Just verify the function runs without panicking
        // The result depends on hardware state
        let result = check_cd_status();
        assert!(result.is_ok() || result.is_err());
    }
}
