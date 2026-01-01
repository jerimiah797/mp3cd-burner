//! CD burning using hdiutil/drutil
//! (Future: Stage 10)
#![allow(dead_code)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};

/// Progress callback type for burn operations
pub type ProgressCallback = Box<dyn Fn(i32) + Send>;

/// Check if a blank CD is inserted
pub fn check_cd_inserted() -> Result<bool, String> {
    let output = Command::new("drutil")
        .args(["status"])
        .output()
        .map_err(|e| format!("Failed to execute drutil: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check for blank media
    let has_blank_cd = stdout.contains("Blank") || stdout.contains("blank");

    Ok(has_blank_cd)
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
    if !iso_path.exists() {
        return Err(format!("ISO file not found: {}", iso_path.display()));
    }

    println!("Starting burn of {}", iso_path.display());

    // Spawn hdiutil with -puppetstrings to get progress output
    let mut child = Command::new("hdiutil")
        .args([
            "burn",
            "-noverifyburn",
            "-puppetstrings",
            iso_path.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to execute hdiutil burn: {}", e))?;

    // Read stdout for progress updates
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            if let Ok(line) = line {
                // Parse progress lines like "PERCENT:0.059725" or "PERCENT:-1.000000"
                if line.starts_with("PERCENT:") {
                    if let Some(percent_str) = line.strip_prefix("PERCENT:") {
                        if let Ok(percentage_float) = percent_str.trim().parse::<f64>() {
                            let percentage = percentage_float.round() as i32;
                            if let Some(ref callback) = on_progress {
                                callback(percentage);
                            }
                        }
                    }
                }
            }
        }
    }

    // Wait for completion
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for hdiutil burn: {}", e))?;

    if output.status.success() {
        println!("Burn completed successfully");
        Ok(())
    } else {
        let error_msg = String::from_utf8_lossy(&output.stderr);
        Err(format!("Failed to burn CD: {}", error_msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_cd_inserted_runs() {
        // Just verify the function runs without panicking
        // The result depends on hardware state
        let result = check_cd_inserted();
        assert!(result.is_ok() || result.is_err());
    }
}
