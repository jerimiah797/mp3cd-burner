//! Bitrate optimization through dry-run estimation
//!
//! Estimates output size without running ffmpeg, then iteratively
//! adjusts target bitrate to maximize quality while staying under CD capacity.
//!
//! Note: This module is kept for reference but the multi-pass approach in
//! folder_list.rs provides better accuracy by measuring actual output sizes.

#![allow(dead_code)]

use crate::audio::{determine_encoding_strategy, EncodingStrategy};
use crate::core::AudioFileInfo;

/// CD capacity target in bytes (685 MB)
const CD_CAPACITY_BYTES: u64 = 685 * 1024 * 1024;

/// Safety margin for estimation errors (5%)
/// Accounts for: VBR encoding unpredictability at higher bitrates,
/// album art inflation in bitrate calculation, MP3 vs AAC efficiency differences
const SAFETY_MARGIN: f64 = 0.05;

/// Maximum bitrate to consider (kbps)
const MAX_BITRATE: u32 = 320;

/// Minimum bitrate to consider (kbps)
const MIN_BITRATE: u32 = 64;

/// Bitrate increment for optimization iterations (kbps)
const BITRATE_STEP: u32 = 8;

/// Result of estimating a single file's output size
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FileEstimate {
    pub strategy: EncodingStrategy,
    pub estimated_bytes: u64,
    pub duration_secs: f64,
}

/// Result of estimating total output for a set of files
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConversionEstimate {
    pub target_bitrate: u32,
    pub total_bytes: u64,
    pub copy_count: usize,
    pub transcode_count: usize,
    pub headroom_bytes: i64,
}

impl ConversionEstimate {
    /// Returns headroom as MB (positive = under capacity, negative = over)
    pub fn headroom_mb(&self) -> f64 {
        self.headroom_bytes as f64 / (1024.0 * 1024.0)
    }
}

/// Estimate output size for a single file based on its encoding strategy
pub fn estimate_file_size(
    file: &AudioFileInfo,
    target_bitrate: u32,
) -> FileEstimate {
    let strategy = determine_encoding_strategy(
        &file.codec,
        file.bitrate,
        target_bitrate,
        file.is_lossy,
        false, // no_lossy_mode
        false, // embed_album_art (we strip for CD burning)
    );

    let estimated_bytes = match &strategy {
        EncodingStrategy::Copy => {
            // Exact size - we're copying the file as-is
            file.size
        }
        EncodingStrategy::CopyWithoutArt => {
            // Slightly smaller - stripping album art
            // Estimate ~100KB savings for embedded art, but don't go below audio size
            let audio_estimate = (file.duration * file.bitrate as f64 * 1000.0 / 8.0) as u64;
            file.size.min(audio_estimate + 50_000) // Conservative: keep some overhead
        }
        EncodingStrategy::ConvertAtSourceBitrate(br) |
        EncodingStrategy::ConvertAtTargetBitrate(br) => {
            // Estimate: duration * bitrate / 8 + small overhead for MP3 framing
            let audio_bytes = (file.duration * *br as f64 * 1000.0 / 8.0) as u64;
            audio_bytes + 10_000 // ~10KB overhead for headers/padding
        }
    };

    FileEstimate {
        strategy,
        estimated_bytes,
        duration_secs: file.duration,
    }
}

/// Estimate total output size for all files at a given target bitrate
pub fn estimate_conversion(
    files: &[AudioFileInfo],
    target_bitrate: u32,
) -> ConversionEstimate {
    let mut total_bytes = 0u64;
    let mut copy_count = 0usize;
    let mut transcode_count = 0usize;

    for file in files {
        let estimate = estimate_file_size(file, target_bitrate);
        total_bytes += estimate.estimated_bytes;

        match estimate.strategy {
            EncodingStrategy::Copy | EncodingStrategy::CopyWithoutArt => copy_count += 1,
            _ => transcode_count += 1,
        }
    }

    // Apply safety margin to account for estimation errors
    let adjusted_bytes = (total_bytes as f64 * (1.0 + SAFETY_MARGIN)) as u64;
    let headroom_bytes = CD_CAPACITY_BYTES as i64 - adjusted_bytes as i64;

    ConversionEstimate {
        target_bitrate,
        total_bytes: adjusted_bytes,
        copy_count,
        transcode_count,
        headroom_bytes,
    }
}

/// Optimize bitrate to maximize quality while staying under CD capacity
///
/// Returns the optimal bitrate and the estimate at that bitrate.
/// Starts from the initial bitrate and increases until we exceed capacity,
/// then backs off to the last safe value.
pub fn optimize_bitrate(
    files: &[AudioFileInfo],
    initial_bitrate: u32,
) -> (u32, ConversionEstimate) {
    let mut best_bitrate = initial_bitrate.clamp(MIN_BITRATE, MAX_BITRATE);
    let mut best_estimate = estimate_conversion(files, best_bitrate);

    // If initial estimate is already over capacity, decrease bitrate
    if best_estimate.headroom_bytes < 0 {
        while best_bitrate > MIN_BITRATE && best_estimate.headroom_bytes < 0 {
            best_bitrate = best_bitrate.saturating_sub(BITRATE_STEP);
            best_estimate = estimate_conversion(files, best_bitrate);
        }
        return (best_bitrate, best_estimate);
    }

    // We have headroom - try increasing bitrate to use it
    let mut current_bitrate = best_bitrate;

    while current_bitrate < MAX_BITRATE {
        let next_bitrate = (current_bitrate + BITRATE_STEP).min(MAX_BITRATE);
        let next_estimate = estimate_conversion(files, next_bitrate);

        if next_estimate.headroom_bytes >= 0 {
            // Still fits - this becomes our new best
            best_bitrate = next_bitrate;
            best_estimate = next_estimate;
            current_bitrate = next_bitrate;
        } else {
            // Exceeded capacity - stop here
            break;
        }
    }

    println!(
        "Bitrate optimization: {} -> {} kbps (headroom: {:.1} MB)",
        initial_bitrate,
        best_bitrate,
        best_estimate.headroom_mb()
    );

    (best_bitrate, best_estimate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_test_file(codec: &str, bitrate: u32, duration: f64, size: u64, is_lossy: bool) -> AudioFileInfo {
        AudioFileInfo {
            path: PathBuf::from("/test/file.mp3"),
            duration,
            bitrate,
            size,
            codec: codec.to_string(),
            is_lossy,
        }
    }

    #[test]
    fn test_estimate_copy_file() {
        // MP3 at 128kbps, target 256kbps - should copy without art
        let file = make_test_file("mp3", 128, 180.0, 3_000_000, true);
        let estimate = estimate_file_size(&file, 256);

        assert!(matches!(estimate.strategy, EncodingStrategy::CopyWithoutArt));
        // CopyWithoutArt estimates: min(file_size, audio_estimate + 50KB)
        // audio_estimate = 180s * 128kbps * 1000 / 8 = 2,880,000 + 50,000 = 2,930,000
        assert_eq!(estimate.estimated_bytes, 2_930_000);
    }

    #[test]
    fn test_estimate_transcode_file() {
        // FLAC file - should transcode
        let file = make_test_file("flac", 0, 180.0, 30_000_000, false);
        let estimate = estimate_file_size(&file, 256);

        assert!(matches!(estimate.strategy, EncodingStrategy::ConvertAtTargetBitrate(256)));
        // 180 seconds * 256 kbps * 1000 / 8 = 5,760,000 bytes + overhead
        assert!(estimate.estimated_bytes > 5_700_000);
        assert!(estimate.estimated_bytes < 6_000_000);
    }

    #[test]
    fn test_estimate_conversion_total() {
        let files = vec![
            make_test_file("mp3", 128, 180.0, 3_000_000, true),  // Copy
            make_test_file("flac", 0, 180.0, 30_000_000, false), // Transcode
        ];

        let estimate = estimate_conversion(&files, 256);

        assert_eq!(estimate.copy_count, 1);
        assert_eq!(estimate.transcode_count, 1);
        assert!(estimate.total_bytes > 8_000_000);
    }

    #[test]
    fn test_optimize_bitrate_increases() {
        // Small files that easily fit - should bump bitrate up
        let files = vec![
            make_test_file("flac", 0, 60.0, 10_000_000, false),
            make_test_file("flac", 0, 60.0, 10_000_000, false),
        ];

        let (optimized, estimate) = optimize_bitrate(&files, 128);

        // Should increase from 128 since files are small
        assert!(optimized > 128);
        assert!(estimate.headroom_bytes >= 0);
    }

    #[test]
    fn test_optimize_bitrate_decreases_when_over() {
        // Simulate ~6 hours of audio that won't fit at high bitrate
        // At 320kbps: 320 * 1000 / 8 = 40,000 bytes/sec
        // CD capacity: 685 * 1024 * 1024 = 718,274,560 bytes
        // Max duration at 320kbps: 718,274,560 / 40,000 = 17,957 seconds = ~5 hours
        // So 10 files of 36 minutes each = 6 hours should exceed capacity
        let files: Vec<_> = (0..10)
            .map(|_| make_test_file("flac", 0, 2160.0, 50_000_000, false)) // 36 min each
            .collect();

        let (optimized, estimate) = optimize_bitrate(&files, 320);

        // Should decrease since total duration exceeds 5 hours
        assert!(optimized < 320, "Expected bitrate < 320, got {}", optimized);
        assert!(estimate.headroom_bytes >= 0);
    }
}
