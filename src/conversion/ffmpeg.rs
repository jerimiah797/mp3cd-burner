//! FFmpeg subprocess handling for audio conversion

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Result of a file conversion
#[derive(Debug, Clone)]
pub struct ConversionResult {
    /// Path to the converted output file
    pub output_path: PathBuf,
    /// Original input file path
    pub input_path: PathBuf,
    /// Whether conversion was successful
    pub success: bool,
    /// Error message if conversion failed
    pub error: Option<String>,
}

/// Convert a single audio file to MP3 using ffmpeg
///
/// # Arguments
/// * `ffmpeg_path` - Path to the ffmpeg binary
/// * `input_path` - Path to the input audio file
/// * `output_path` - Path for the output MP3 file
/// * `bitrate` - Target bitrate in kbps (e.g., 256)
///
/// # Returns
/// Result indicating success or failure with details
pub fn convert_file(
    ffmpeg_path: &Path,
    input_path: &Path,
    output_path: &Path,
    bitrate: u32,
) -> ConversionResult {
    // Build ffmpeg arguments
    // -i <input>        : Input file
    // -vn               : Skip video streams (album art) - simplifies initial impl
    // -codec:a libmp3lame : Use LAME MP3 encoder
    // -b:a <bitrate>k   : Set audio bitrate
    // -y                : Overwrite output file without asking
    let bitrate_str = format!("{}k", bitrate);
    let input_str = input_path.to_str().unwrap_or("");
    let output_str = output_path.to_str().unwrap_or("");

    let args = vec![
        "-i",
        input_str,
        "-vn",
        "-codec:a",
        "libmp3lame",
        "-b:a",
        &bitrate_str,
        "-y",
        output_str,
    ];

    println!(
        "Converting: {} -> {} at {}kbps",
        input_path.display(),
        output_path.display(),
        bitrate
    );

    // Spawn ffmpeg process
    let result = Command::new(ffmpeg_path)
        .args(&args)
        .output();

    match result {
        Ok(output) => {
            if output.status.success() {
                println!("Successfully converted: {}", input_path.display());
                ConversionResult {
                    output_path: output_path.to_path_buf(),
                    input_path: input_path.to_path_buf(),
                    success: true,
                    error: None,
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let error_msg = format!(
                    "ffmpeg exited with status {}: {}",
                    output.status,
                    stderr.lines().last().unwrap_or("Unknown error")
                );
                println!("Conversion failed: {}", error_msg);
                ConversionResult {
                    output_path: output_path.to_path_buf(),
                    input_path: input_path.to_path_buf(),
                    success: false,
                    error: Some(error_msg),
                }
            }
        }
        Err(e) => {
            let error_msg = format!("Failed to spawn ffmpeg: {}", e);
            println!("Conversion error: {}", error_msg);
            ConversionResult {
                output_path: output_path.to_path_buf(),
                input_path: input_path.to_path_buf(),
                success: false,
                error: Some(error_msg),
            }
        }
    }
}

/// Convert a file, creating the output directory if needed
pub fn convert_file_with_mkdir(
    ffmpeg_path: &Path,
    input_path: &Path,
    output_dir: &Path,
    bitrate: u32,
) -> ConversionResult {
    // Create output directory if it doesn't exist
    if !output_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(output_dir) {
            return ConversionResult {
                output_path: PathBuf::new(),
                input_path: input_path.to_path_buf(),
                success: false,
                error: Some(format!("Failed to create output directory: {}", e)),
            };
        }
    }

    // Generate output filename (same name but .mp3 extension)
    let file_stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let output_path = output_dir.join(format!("{}.mp3", file_stem));

    convert_file(ffmpeg_path, input_path, &output_path, bitrate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversion_result_creation() {
        let result = ConversionResult {
            output_path: PathBuf::from("/tmp/test.mp3"),
            input_path: PathBuf::from("/home/user/song.flac"),
            success: true,
            error: None,
        };

        assert!(result.success);
        assert!(result.error.is_none());
    }
}
