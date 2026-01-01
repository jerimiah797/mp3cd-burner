//! FFmpeg subprocess handling for audio conversion

use std::path::PathBuf;

/// Result of a file conversion
#[derive(Debug, Clone)]
pub struct ConversionResult {
    /// Path to the converted output file
    #[allow(dead_code)]
    pub output_path: PathBuf,
    /// Original input file path
    #[allow(dead_code)]
    pub input_path: PathBuf,
    /// Whether conversion was successful
    pub success: bool,
    /// Error message if conversion failed
    pub error: Option<String>,
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
