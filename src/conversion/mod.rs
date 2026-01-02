//! Audio conversion module
//!
//! Handles transcoding audio files to MP3 using ffmpeg.

mod background;
mod ffmpeg;
mod optimizer;
mod output_manager;
mod parallel;

pub use background::{
    BackgroundEncoder, BackgroundEncoderHandle, EncoderEvent,
};
pub use ffmpeg::ConversionResult;
pub use optimizer::{calculate_multipass_bitrate, MultipassEstimate};
pub use output_manager::OutputManager;

use std::path::PathBuf;

/// Get the path to the bundled ffmpeg binary
///
/// In development, looks for ffmpeg at CARGO_MANIFEST_DIR/resources/bin/ffmpeg
/// In release builds, will need to be bundled with the app.
pub fn get_ffmpeg_path() -> Result<PathBuf, String> {
    // Try CARGO_MANIFEST_DIR first (development mode)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let dev_path = PathBuf::from(manifest_dir)
            .join("resources")
            .join("bin")
            .join("ffmpeg");

        if dev_path.exists() {
            println!("Found ffmpeg at development path: {:?}", dev_path);
            return Ok(dev_path);
        }
    }

    // Try relative to current executable (release mode)
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            // macOS app bundle: Contents/MacOS/../Resources/bin/ffmpeg
            let bundle_path = exe_dir
                .join("..")
                .join("Resources")
                .join("bin")
                .join("ffmpeg");

            if bundle_path.exists() {
                println!("Found ffmpeg at bundle path: {:?}", bundle_path);
                return Ok(bundle_path);
            }

            // Also try directly next to executable
            let local_path = exe_dir.join("resources").join("bin").join("ffmpeg");
            if local_path.exists() {
                println!("Found ffmpeg at local path: {:?}", local_path);
                return Ok(local_path);
            }
        }
    }

    Err("ffmpeg binary not found. Expected at resources/bin/ffmpeg".to_string())
}

/// Verify that ffmpeg exists and is executable
pub fn verify_ffmpeg() -> Result<PathBuf, String> {
    let path = get_ffmpeg_path()?;

    // Check if file exists
    if !path.exists() {
        return Err(format!("ffmpeg not found at {:?}", path));
    }

    // On Unix, check if executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&path)
            .map_err(|e| format!("Failed to get ffmpeg metadata: {}", e))?;
        let permissions = metadata.permissions();
        if permissions.mode() & 0o111 == 0 {
            return Err(format!("ffmpeg at {:?} is not executable", path));
        }
    }

    println!("ffmpeg verified at: {:?}", path);
    Ok(path)
}

/// Get the output directory for converted files
pub fn get_output_dir() -> PathBuf {
    std::env::temp_dir().join("mp3cd_output")
}

/// Create the output directory if it doesn't exist
pub fn ensure_output_dir() -> Result<PathBuf, String> {
    let output_dir = get_output_dir();

    if output_dir.exists() {
        // Clean existing output
        std::fs::remove_dir_all(&output_dir)
            .map_err(|e| format!("Failed to clean output directory: {}", e))?;
    }

    std::fs::create_dir_all(&output_dir)
        .map_err(|e| format!("Failed to create output directory: {}", e))?;

    println!("Output directory ready: {:?}", output_dir);
    Ok(output_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_ffmpeg_path() {
        let result = get_ffmpeg_path();
        assert!(result.is_ok(), "Should find ffmpeg in development mode");

        let path = result.unwrap();
        assert!(path.exists(), "ffmpeg path should exist");
    }

    #[test]
    fn test_verify_ffmpeg() {
        let result = verify_ffmpeg();
        assert!(result.is_ok(), "ffmpeg should be verified");
    }

    #[test]
    fn test_get_output_dir() {
        let output_dir = get_output_dir();
        assert!(output_dir.to_string_lossy().contains("mp3cd_output"));
    }
}
