//! Bitrate calculation for CD-fitting optimization
//!
//! This module implements smart bitrate calculation to fit audio content
//! onto a 700MB CD. It uses an iterative approach to find the optimal
//! encoding bitrate while minimizing quality loss.
//! (Future: Stage 6)
#![allow(dead_code)]

use super::AudioFileInfo;

/// Target CD size in bytes (700 MB decimal)
/// CD-Rs are labeled 700 MB using decimal (not binary) megabytes
pub const TARGET_SIZE_BYTES: u64 = 700 * 1000 * 1000;

/// Maximum MP3 bitrate (kbps)
pub const MAX_BITRATE: u32 = 320;

/// Minimum MP3 bitrate (kbps)
pub const MIN_BITRATE: u32 = 64;

/// MP3 encoding overhead multiplier (accounts for framing, headers, etc.)
const MP3_OVERHEAD_MULTIPLIER: f64 = 1.065;

/// Maximum iterations for bitrate refinement
const MAX_ITERATIONS: usize = 5;

/// Overhead compensation factor for conservative size estimates
const OVERHEAD_COMPENSATION: f64 = 0.80;

/// Result of bitrate calculation
#[derive(Debug, Clone)]
pub struct BitrateCalculation {
    /// Recommended target bitrate in kbps
    pub target_bitrate: u32,
    /// Estimated output size in bytes
    pub estimated_size: u64,
    /// Files that will be copied as-is (MP3s under target bitrate)
    pub files_to_copy: Vec<usize>,
    /// Files that need to be converted
    pub files_to_convert: Vec<usize>,
    /// Whether the content fits on the CD
    pub fits_on_cd: bool,
}

/// Encoding decision for a single file
#[derive(Debug, Clone, PartialEq)]
pub enum EncodingDecision {
    /// Copy file as-is (no transcoding needed)
    Copy,
    /// Convert at the specified bitrate
    ConvertAt(u32),
}

/// Calculate optimal bitrate to fit all files on a 700MB CD
///
/// This uses an iterative approach:
/// 1. Start with an initial bitrate estimate based on total duration
/// 2. Calculate which MP3s can be copied (bitrate <= target)
/// 3. Refine the bitrate based on space used by copied files
/// 4. Repeat until convergence or max iterations
///
/// # Arguments
/// * `files` - List of audio files with metadata
/// * `no_lossy_conversions` - If true, never re-encode lossy files (only convert lossless)
///
/// # Returns
/// A `BitrateCalculation` with the recommended bitrate and file decisions
pub fn calculate_optimal_bitrate(
    files: &[AudioFileInfo],
    no_lossy_conversions: bool,
) -> BitrateCalculation {
    if files.is_empty() {
        return BitrateCalculation {
            target_bitrate: MAX_BITRATE,
            estimated_size: 0,
            files_to_copy: vec![],
            files_to_convert: vec![],
            fits_on_cd: true,
        };
    }

    let total_duration: f64 = files.iter().map(|f| f.duration).sum();
    let target_size = (TARGET_SIZE_BYTES as f64 * OVERHEAD_COMPENSATION) as u64;

    // Initial bitrate estimate based on total duration
    // bitrate (kbps) = (size_bytes * 8) / duration_seconds / 1000
    let mut target_bitrate = ((target_size * 8) as f64 / total_duration / 1000.0) as u32;
    target_bitrate = target_bitrate.clamp(MIN_BITRATE, MAX_BITRATE);

    // Iteratively refine the bitrate
    for _ in 0..MAX_ITERATIONS {
        let (_, _, copy_size, convert_duration) =
            categorize_files(files, target_bitrate, no_lossy_conversions);

        // Calculate how much space is left for converted files
        let space_for_converts = target_size.saturating_sub(copy_size);

        if convert_duration > 0.0 {
            // Recalculate bitrate for remaining files
            let new_bitrate =
                ((space_for_converts * 8) as f64 / convert_duration / 1000.0) as u32;
            let new_bitrate = new_bitrate.clamp(MIN_BITRATE, MAX_BITRATE);

            // Check for convergence
            if new_bitrate == target_bitrate {
                break;
            }
            target_bitrate = new_bitrate;
        } else {
            // All files can be copied
            break;
        }
    }

    // Final categorization with the computed bitrate
    let (files_to_copy, files_to_convert, _, _) =
        categorize_files(files, target_bitrate, no_lossy_conversions);

    let estimated_size =
        calculate_estimated_output_size(files, target_bitrate, no_lossy_conversions);

    BitrateCalculation {
        target_bitrate,
        estimated_size,
        files_to_copy,
        files_to_convert,
        fits_on_cd: estimated_size <= TARGET_SIZE_BYTES,
    }
}

/// Categorize files into copy vs convert, with size and duration totals
///
/// Returns: (files_to_copy indices, files_to_convert indices, total_copy_size, total_convert_duration)
fn categorize_files(
    files: &[AudioFileInfo],
    target_bitrate: u32,
    no_lossy_conversions: bool,
) -> (Vec<usize>, Vec<usize>, u64, f64) {
    let mut files_to_copy = Vec::new();
    let mut files_to_convert = Vec::new();
    let mut copy_size: u64 = 0;
    let mut convert_duration: f64 = 0.0;

    for (i, file) in files.iter().enumerate() {
        let decision = get_encoding_decision(file, target_bitrate, no_lossy_conversions);
        match decision {
            EncodingDecision::Copy => {
                files_to_copy.push(i);
                copy_size += file.size;
            }
            EncodingDecision::ConvertAt(_) => {
                files_to_convert.push(i);
                convert_duration += file.duration;
            }
        }
    }

    (files_to_copy, files_to_convert, copy_size, convert_duration)
}

/// Determine the encoding decision for a single file
///
/// Logic:
/// - MP3 files: copy if bitrate <= target + 20 threshold, otherwise convert at source
/// - Other lossy files: if no_lossy_conversions, copy; otherwise convert at source bitrate
/// - Lossless files: always convert at target bitrate
///
/// NOTE: This must match the behavior in audio/conversion.rs:determine_encoding_strategy()
/// Lossy files are encoded at their source bitrate to preserve quality (not capped at target).
pub fn get_encoding_decision(
    file: &AudioFileInfo,
    target_bitrate: u32,
    no_lossy_conversions: bool,
) -> EncodingDecision {
    let is_mp3 = file.codec.to_lowercase() == "mp3";

    // Copy threshold for MP3s (matches audio/conversion.rs)
    const COPY_THRESHOLD: u32 = 20;

    if is_mp3 {
        // MP3 files: copy if within threshold of target
        if file.bitrate <= target_bitrate + COPY_THRESHOLD {
            EncodingDecision::Copy
        } else {
            // High-bitrate MP3s are transcoded at source bitrate (capped at 320)
            EncodingDecision::ConvertAt(file.bitrate.min(320))
        }
    } else if file.is_lossy {
        // Other lossy formats (AAC, OGG, etc.)
        if no_lossy_conversions {
            // Never re-encode lossy files
            EncodingDecision::Copy
        } else {
            // Convert at source bitrate to preserve quality (capped at 320 for MP3 limits)
            // This matches audio/conversion.rs behavior
            EncodingDecision::ConvertAt(file.bitrate.min(320))
        }
    } else {
        // Lossless files (FLAC, WAV, ALAC): always convert at target
        EncodingDecision::ConvertAt(target_bitrate)
    }
}

/// Calculate the estimated output size for all files
///
/// Takes into account:
/// - Files that will be copied as-is keep their original size
/// - Files that will be converted use bitrate * duration calculation with overhead
pub fn calculate_estimated_output_size(
    files: &[AudioFileInfo],
    target_bitrate: u32,
    no_lossy_conversions: bool,
) -> u64 {
    let mut total_size: f64 = 0.0;

    for file in files {
        let decision = get_encoding_decision(file, target_bitrate, no_lossy_conversions);
        match decision {
            EncodingDecision::Copy => {
                total_size += file.size as f64;
            }
            EncodingDecision::ConvertAt(bitrate) => {
                // size = bitrate (kbps) * duration (s) / 8 * 1000 (convert kbps to bps)
                // = bitrate * duration * 125 (bytes)
                let encoded_size = (bitrate as f64) * file.duration * 125.0;
                total_size += encoded_size * MP3_OVERHEAD_MULTIPLIER;
            }
        }
    }

    total_size as u64
}

/// Check if the files will fit on a CD at the given bitrate
pub fn will_fit_on_cd(files: &[AudioFileInfo], target_bitrate: u32, no_lossy_conversions: bool) -> bool {
    let estimated_size = calculate_estimated_output_size(files, target_bitrate, no_lossy_conversions);
    estimated_size <= TARGET_SIZE_BYTES
}

/// Format bitrate for display (e.g., "320 kbps")
pub fn format_bitrate(kbps: u32) -> String {
    format!("{} kbps", kbps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_mp3(bitrate: u32, duration: f64) -> AudioFileInfo {
        let size = (bitrate as u64 * duration as u64 * 125) as u64;
        AudioFileInfo {
            path: PathBuf::from("/test/song.mp3"),
            duration,
            bitrate,
            size,
            codec: "mp3".to_string(),
            is_lossy: true,
        }
    }

    fn make_flac(duration: f64) -> AudioFileInfo {
        // FLAC is typically around 800-1000 kbps
        let bitrate = 900;
        let size = (bitrate as u64 * duration as u64 * 125) as u64;
        AudioFileInfo {
            path: PathBuf::from("/test/song.flac"),
            duration,
            bitrate,
            size,
            codec: "flac".to_string(),
            is_lossy: false,
        }
    }

    fn make_aac(bitrate: u32, duration: f64) -> AudioFileInfo {
        let size = (bitrate as u64 * duration as u64 * 125) as u64;
        AudioFileInfo {
            path: PathBuf::from("/test/song.m4a"),
            duration,
            bitrate,
            size,
            codec: "aac".to_string(),
            is_lossy: true,
        }
    }

    #[test]
    fn test_empty_files() {
        let result = calculate_optimal_bitrate(&[], false);
        assert_eq!(result.target_bitrate, MAX_BITRATE);
        assert_eq!(result.estimated_size, 0);
        assert!(result.fits_on_cd);
    }

    #[test]
    fn test_mp3_copy_decision() {
        let file = make_mp3(256, 180.0);
        let decision = get_encoding_decision(&file, 320, false);
        assert_eq!(decision, EncodingDecision::Copy);
    }

    #[test]
    fn test_mp3_convert_decision() {
        // MP3 at 320 kbps, target is 256 kbps
        // 320 > 256 + 20 (threshold), so it converts
        // Lossy files convert at SOURCE bitrate (not target) to preserve quality
        let file = make_mp3(320, 180.0);
        let decision = get_encoding_decision(&file, 256, false);
        assert_eq!(decision, EncodingDecision::ConvertAt(320));
    }

    #[test]
    fn test_flac_always_converts() {
        let file = make_flac(180.0);
        let decision = get_encoding_decision(&file, 256, false);
        assert_eq!(decision, EncodingDecision::ConvertAt(256));

        // Even with no_lossy_conversions, lossless still converts
        let decision2 = get_encoding_decision(&file, 256, true);
        assert_eq!(decision2, EncodingDecision::ConvertAt(256));
    }

    #[test]
    fn test_aac_no_lossy_mode() {
        let file = make_aac(256, 180.0);

        // Without no_lossy mode, AAC converts at SOURCE bitrate (not target)
        // to preserve quality (avoid lossy-to-lossy quality degradation)
        let decision = get_encoding_decision(&file, 192, false);
        assert_eq!(decision, EncodingDecision::ConvertAt(256));

        // With no_lossy mode, AAC copies
        let decision2 = get_encoding_decision(&file, 192, true);
        assert_eq!(decision2, EncodingDecision::Copy);
    }

    #[test]
    fn test_aac_uses_min_bitrate() {
        // AAC at 192 kbps, target is 256
        let file = make_aac(192, 180.0);
        let decision = get_encoding_decision(&file, 256, false);
        // Should use source bitrate (192) not target (256)
        assert_eq!(decision, EncodingDecision::ConvertAt(192));
    }

    #[test]
    fn test_small_album_fits() {
        // 10 songs at 4 minutes each = 40 minutes total
        // At 320 kbps: 320 * 40 * 60 * 125 = ~96 MB
        let files: Vec<AudioFileInfo> = (0..10).map(|_| make_mp3(320, 240.0)).collect();
        let result = calculate_optimal_bitrate(&files, false);

        assert!(result.fits_on_cd);
        assert_eq!(result.target_bitrate, MAX_BITRATE);
    }

    #[test]
    fn test_large_album_needs_lower_bitrate() {
        // 100 songs at 5 minutes each = 500 minutes = 30000 seconds
        // At 320 kbps: 320 * 30000 * 125 = ~1.2 GB (doesn't fit)
        let files: Vec<AudioFileInfo> = (0..100).map(|_| make_flac(300.0)).collect();
        let result = calculate_optimal_bitrate(&files, false);

        // Should calculate a lower bitrate to fit
        assert!(result.target_bitrate < MAX_BITRATE);
        assert!(result.fits_on_cd);
    }

    #[test]
    fn test_estimated_size_calculation() {
        // Single 3-minute MP3 at 320 kbps
        let file = make_mp3(320, 180.0);

        // Copy decision - should use original size
        let size_copy = calculate_estimated_output_size(&[file.clone()], 320, false);
        assert_eq!(size_copy, file.size);

        // With target 256 kbps, MP3 at 320 kbps (320 > 256 + 20 threshold)
        // converts at SOURCE bitrate (320) to preserve quality
        let size_convert = calculate_estimated_output_size(&[file], 256, false);
        // 320 * 180 * 125 * 1.065 = ~7.67 MB (uses source bitrate, not target)
        let expected = (320.0 * 180.0 * 125.0 * MP3_OVERHEAD_MULTIPLIER) as u64;
        assert_eq!(size_convert, expected);
    }

    #[test]
    fn test_format_bitrate() {
        assert_eq!(format_bitrate(320), "320 kbps");
        assert_eq!(format_bitrate(128), "128 kbps");
    }

    #[test]
    fn test_will_fit_on_cd() {
        // Small file definitely fits
        let small_file = make_mp3(128, 180.0);
        assert!(will_fit_on_cd(&[small_file], 320, false));

        // Create files that definitely won't fit
        // 1000 songs at 10 minutes each at FLAC = way over 700MB
        let huge_files: Vec<AudioFileInfo> = (0..1000).map(|_| make_flac(600.0)).collect();
        // Even at min bitrate, might still not fit
        let fits = will_fit_on_cd(&huge_files, MIN_BITRATE, false);
        // This is expected to not fit
        assert!(!fits);
    }
}
