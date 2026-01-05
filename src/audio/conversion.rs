//! Audio conversion module - handles encoding strategy and file processing
//! (Future: Stage 6)
#![allow(dead_code)]

/// Represents different encoding strategies for audio files
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodingStrategy {
    /// Copy MP3 file as-is (preserves album art)
    Copy,
    /// Copy MP3 file but strip album art
    CopyWithoutArt,
    /// Convert lossy file at source bitrate (for non-MP3 lossy formats)
    ConvertAtSourceBitrate(u32),
    /// Convert lossless file at target bitrate
    ConvertAtTargetBitrate(u32),
}

/// Determines the appropriate encoding strategy based on file metadata and settings
///
/// # Arguments
/// * `codec` - The codec of the source file (e.g., "mp3", "flac", "aac")
/// * `source_bitrate` - The bitrate of the source file in kbps
/// * `target_bitrate` - The desired output bitrate in kbps
/// * `is_lossy` - Whether the source file uses lossy compression
/// * `no_lossy_mode` - If true, avoid lossy-to-lossy conversions
/// * `embed_album_art` - If true, preserve/embed album art in output
///
/// # Returns
/// The optimal `EncodingStrategy` for this file
pub fn determine_encoding_strategy(
    codec: &str,
    source_bitrate: u32,
    target_bitrate: u32,
    is_lossy: bool,
    no_lossy_mode: bool,
    embed_album_art: bool,
) -> EncodingStrategy {
    if no_lossy_mode {
        // No lossy conversions mode: avoid lossy-to-lossy conversions
        if codec == "mp3" {
            // MP3 files are copied to preserve quality
            if embed_album_art {
                EncodingStrategy::Copy
            } else {
                EncodingStrategy::CopyWithoutArt
            }
        } else if is_lossy {
            // Convert lossy non-MP3 (AAC, OGG, etc.) at source bitrate
            // This minimizes quality loss from double compression
            EncodingStrategy::ConvertAtSourceBitrate(source_bitrate)
        } else {
            // Lossless formats (FLAC, WAV) - convert at target bitrate
            EncodingStrategy::ConvertAtTargetBitrate(target_bitrate)
        }
    } else {
        // Normal mode: optimize for file size while preserving quality
        // Copy threshold: don't re-encode MP3s within 20kbps of target
        // This accounts for album art inflating our file-size-based bitrate calculation
        // and avoids quality loss for marginal space savings
        const COPY_THRESHOLD: u32 = 20;
        if codec == "mp3" && source_bitrate <= target_bitrate + COPY_THRESHOLD {
            // MP3 at or near target bitrate - copy to preserve quality
            if embed_album_art {
                EncodingStrategy::Copy
            } else {
                EncodingStrategy::CopyWithoutArt
            }
        } else if is_lossy {
            // Lossy formats (AAC, OGG, OPUS, and high-bitrate MP3s)
            // Transcode at source bitrate to preserve quality
            // Don't cap at target_bitrate - that's for lossless files only
            // Exception: if source > 320, cap at 320 (max MP3 bitrate)
            let capped_bitrate = source_bitrate.min(320);
            EncodingStrategy::ConvertAtSourceBitrate(capped_bitrate)
        } else {
            // Lossless formats (FLAC, WAV, ALAC, etc.) - convert at target bitrate
            EncodingStrategy::ConvertAtTargetBitrate(target_bitrate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== No Lossy Mode Tests =====

    #[test]
    fn test_no_lossy_mode_mp3_with_album_art() {
        // In no lossy mode, MP3 files should be copied to preserve quality
        // If embed_album_art is true, use Copy to preserve art
        let strategy = determine_encoding_strategy(
            "mp3", 192, 256, true, true, // no_lossy_mode
            true, // embed_album_art
        );
        assert_eq!(strategy, EncodingStrategy::Copy);
    }

    #[test]
    fn test_no_lossy_mode_mp3_without_album_art() {
        // In no lossy mode, MP3 files should be copied
        // If embed_album_art is false, use CopyWithoutArt to strip art
        let strategy = determine_encoding_strategy(
            "mp3", 192, 256, true, true,  // no_lossy_mode
            false, // embed_album_art
        );
        assert_eq!(strategy, EncodingStrategy::CopyWithoutArt);
    }

    #[test]
    fn test_no_lossy_mode_lossy_non_mp3_aac() {
        // AAC is lossy but not MP3 - convert at source bitrate to avoid double loss
        let strategy = determine_encoding_strategy(
            "aac", 192, 256, true, // is_lossy
            true, // no_lossy_mode
            true,
        );
        assert_eq!(strategy, EncodingStrategy::ConvertAtSourceBitrate(192));
    }

    #[test]
    fn test_no_lossy_mode_lossy_non_mp3_opus() {
        // OPUS is lossy but not MP3 - convert at source bitrate
        let strategy = determine_encoding_strategy(
            "opus", 128, 256, true, // is_lossy
            true, // no_lossy_mode
            false,
        );
        assert_eq!(strategy, EncodingStrategy::ConvertAtSourceBitrate(128));
    }

    #[test]
    fn test_no_lossy_mode_lossless_flac() {
        // FLAC is lossless - convert at target bitrate
        let strategy = determine_encoding_strategy(
            "flac", 0, // Bitrate doesn't apply to lossless
            256, false, // is_lossy
            true,  // no_lossy_mode
            true,
        );
        assert_eq!(strategy, EncodingStrategy::ConvertAtTargetBitrate(256));
    }

    #[test]
    fn test_no_lossy_mode_lossless_wav() {
        // WAV is lossless - convert at target bitrate
        let strategy = determine_encoding_strategy(
            "wav", 0, 320, false, // is_lossy
            true,  // no_lossy_mode
            false,
        );
        assert_eq!(strategy, EncodingStrategy::ConvertAtTargetBitrate(320));
    }

    // ===== Normal Mode (no_lossy_mode = false) Tests =====

    #[test]
    fn test_normal_mode_mp3_below_target_with_art() {
        // MP3 at 192kbps, target 256kbps - copy to preserve quality
        let strategy = determine_encoding_strategy(
            "mp3", 192, 256, true, false, // normal mode
            true,  // embed_album_art
        );
        assert_eq!(strategy, EncodingStrategy::Copy);
    }

    #[test]
    fn test_normal_mode_mp3_below_target_without_art() {
        // MP3 at 192kbps, target 256kbps - copy without art
        let strategy = determine_encoding_strategy(
            "mp3", 192, 256, true, false, // normal mode
            false, // no album art
        );
        assert_eq!(strategy, EncodingStrategy::CopyWithoutArt);
    }

    #[test]
    fn test_normal_mode_mp3_equals_target() {
        // MP3 at 256kbps, target 256kbps - copy it
        let strategy = determine_encoding_strategy("mp3", 256, 256, true, false, true);
        assert_eq!(strategy, EncodingStrategy::Copy);
    }

    #[test]
    fn test_normal_mode_mp3_above_target() {
        // MP3 at 320kbps, target 256kbps - transcode at source bitrate (preserve quality)
        // We don't cap at target_bitrate because that's for lossless files only.
        // Transcoding lossy to lossy at a lower bitrate degrades quality.
        let strategy = determine_encoding_strategy("mp3", 320, 256, true, false, true);
        assert_eq!(strategy, EncodingStrategy::ConvertAtSourceBitrate(320));
    }

    #[test]
    fn test_normal_mode_aac_smart_bitrate_source_lower() {
        // AAC at 128kbps, target 256kbps - use source bitrate (smart bitrate)
        // We want min(source, target) to avoid upsampling lossy
        let strategy = determine_encoding_strategy(
            "aac", 128, 256, true,  // is_lossy
            false, // normal mode
            true,
        );
        assert_eq!(strategy, EncodingStrategy::ConvertAtSourceBitrate(128));
    }

    #[test]
    fn test_normal_mode_aac_high_bitrate() {
        // AAC at 320kbps, target 256kbps - transcode at source bitrate (preserve quality)
        // We don't cap at target_bitrate because that's for lossless files only.
        let strategy = determine_encoding_strategy(
            "aac", 320, 256, true,  // is_lossy
            false, // normal mode
            true,
        );
        assert_eq!(strategy, EncodingStrategy::ConvertAtSourceBitrate(320));
    }

    #[test]
    fn test_normal_mode_ogg_smart_bitrate() {
        // OGG is lossy non-MP3 - use smart bitrate
        let strategy = determine_encoding_strategy("ogg", 192, 256, true, false, false);
        assert_eq!(strategy, EncodingStrategy::ConvertAtSourceBitrate(192));
    }

    #[test]
    fn test_normal_mode_flac() {
        // FLAC is lossless - convert at target bitrate
        let strategy = determine_encoding_strategy("flac", 0, 256, false, false, true);
        assert_eq!(strategy, EncodingStrategy::ConvertAtTargetBitrate(256));
    }

    #[test]
    fn test_normal_mode_wav() {
        // WAV is lossless - convert at target bitrate
        let strategy = determine_encoding_strategy("wav", 0, 320, false, false, false);
        assert_eq!(strategy, EncodingStrategy::ConvertAtTargetBitrate(320));
    }

    // ===== Edge Cases =====

    #[test]
    fn test_very_low_bitrate_mp3() {
        // Very low quality MP3 - should still copy in normal mode
        let strategy = determine_encoding_strategy("mp3", 64, 256, true, false, true);
        assert_eq!(strategy, EncodingStrategy::Copy);
    }

    #[test]
    fn test_very_high_bitrate_mp3() {
        // Very high quality MP3 - transcode at source bitrate (preserve quality)
        // We don't transcode lossy files down to target because:
        // 1. Target bitrate is calculated for lossless files
        // 2. Lossy-to-lossy transcoding at lower bitrate degrades quality
        let strategy = determine_encoding_strategy("mp3", 320, 192, true, false, true);
        assert_eq!(strategy, EncodingStrategy::ConvertAtSourceBitrate(320));
    }

    #[test]
    fn test_unusual_codec_lossy() {
        // Test with WMA (lossy but not MP3)
        let strategy = determine_encoding_strategy("wma", 192, 256, true, false, true);
        // Should use smart bitrate (min of source and target)
        assert_eq!(strategy, EncodingStrategy::ConvertAtSourceBitrate(192));
    }

    #[test]
    fn test_mp3_within_copy_threshold() {
        // MP3 at 170kbps, target 151kbps - within 20kbps threshold, should copy
        // This handles album art inflation and avoids pointless re-encoding
        let strategy = determine_encoding_strategy(
            "mp3", 170, // 170 <= 151 + 20 = 171, so within threshold
            151, true, false, false,
        );
        assert_eq!(strategy, EncodingStrategy::CopyWithoutArt);
    }

    #[test]
    fn test_mp3_above_copy_threshold() {
        // MP3 at 180kbps, target 151kbps - above 20kbps threshold, should transcode
        // But transcode at SOURCE bitrate to preserve quality (not target)
        let strategy = determine_encoding_strategy(
            "mp3", 180, // 180 > 151 + 20 = 171, so above threshold
            151, true, false, true,
        );
        assert_eq!(strategy, EncodingStrategy::ConvertAtSourceBitrate(180));
    }
}
