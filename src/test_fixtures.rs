//! Test fixtures for audio encoding tests
//!
//! This module provides utilities to generate test audio files for testing
//! encoding, metadata reading, and other audio operations.

#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

static FIXTURES_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Get the fixtures directory, creating it if necessary
pub fn fixtures_dir() -> &'static Path {
    FIXTURES_DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join("mp3cd_test_fixtures");
        std::fs::create_dir_all(&dir).expect("Failed to create fixtures directory");
        dir
    })
}

/// Get the path to our bundled ffmpeg
fn ffmpeg_path() -> PathBuf {
    crate::conversion::get_ffmpeg_path().expect("ffmpeg not found")
}

/// Generate a silent audio file of the specified format and duration
///
/// # Arguments
/// * `name` - Base name for the file (without extension)
/// * `format` - Output format: "mp3", "flac", "wav", "aac", "ogg"
/// * `duration_secs` - Duration in seconds
/// * `bitrate` - Bitrate in kbps (for lossy formats)
///
/// # Returns
/// Path to the generated file
pub fn generate_audio_file(
    name: &str,
    format: &str,
    duration_secs: u32,
    bitrate: Option<u32>,
) -> PathBuf {
    let extension = match format {
        "aac" => "m4a",
        other => other,
    };
    let output_path = fixtures_dir().join(format!("{}_{}.{}", name, duration_secs, extension));

    // Return cached file if it exists
    if output_path.exists() {
        return output_path;
    }

    let ffmpeg = ffmpeg_path();

    let mut cmd = Command::new(&ffmpeg);
    cmd.arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(format!(
            "sine=frequency=440:duration={}",
            duration_secs
        ))
        .arg("-y"); // Overwrite output

    // Add format-specific encoding options
    match format {
        "mp3" => {
            cmd.arg("-codec:a").arg("libmp3lame");
            if let Some(br) = bitrate {
                cmd.arg("-b:a").arg(format!("{}k", br));
            }
        }
        "flac" => {
            cmd.arg("-codec:a").arg("flac");
        }
        "wav" => {
            cmd.arg("-codec:a").arg("pcm_s16le");
        }
        "aac" => {
            cmd.arg("-codec:a").arg("aac");
            if let Some(br) = bitrate {
                cmd.arg("-b:a").arg(format!("{}k", br));
            }
        }
        "ogg" => {
            cmd.arg("-codec:a").arg("libvorbis");
            if let Some(br) = bitrate {
                cmd.arg("-b:a").arg(format!("{}k", br));
            }
        }
        _ => panic!("Unsupported format: {}", format),
    }

    cmd.arg(&output_path);

    let output = cmd.output().expect("Failed to execute ffmpeg");

    if !output.status.success() {
        panic!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    output_path
}

/// Generate a set of standard test files (5 seconds each)
pub fn generate_standard_fixtures() -> StandardFixtures {
    StandardFixtures {
        mp3_128: generate_audio_file("test", "mp3", 5, Some(128)),
        mp3_320: generate_audio_file("test", "mp3", 5, Some(320)),
        flac: generate_audio_file("test", "flac", 5, None),
        wav: generate_audio_file("test", "wav", 5, None),
        aac_256: generate_audio_file("test", "aac", 5, Some(256)),
        ogg_192: generate_audio_file("test", "ogg", 5, Some(192)),
    }
}

/// Standard set of test audio files
pub struct StandardFixtures {
    pub mp3_128: PathBuf,
    pub mp3_320: PathBuf,
    pub flac: PathBuf,
    pub wav: PathBuf,
    pub aac_256: PathBuf,
    pub ogg_192: PathBuf,
}

/// Create a test folder structure with audio files
pub fn create_test_album(name: &str, track_count: usize, format: &str) -> PathBuf {
    let album_dir = fixtures_dir().join(name);
    std::fs::create_dir_all(&album_dir).expect("Failed to create album directory");

    for i in 1..=track_count {
        let track_name = format!("{:02} - Track {}", i, i);
        let src = generate_audio_file(&track_name, format, 3, Some(192));
        let dest = album_dir.join(format!("{}.{}", track_name, format));
        if !dest.exists() {
            std::fs::copy(&src, &dest).expect("Failed to copy track");
        }
    }

    album_dir
}

/// Clean up test fixtures
pub fn cleanup_fixtures() {
    let dir = fixtures_dir();
    if dir.exists() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_mp3_file() {
        let path = generate_audio_file("gen_test", "mp3", 2, Some(128));
        assert!(path.exists(), "Generated file should exist");
        assert!(
            std::fs::metadata(&path).unwrap().len() > 0,
            "Generated file should not be empty"
        );
    }

    #[test]
    fn test_generate_flac_file() {
        let path = generate_audio_file("gen_test", "flac", 2, None);
        assert!(path.exists(), "Generated file should exist");
        assert!(
            std::fs::metadata(&path).unwrap().len() > 0,
            "Generated file should not be empty"
        );
    }

    #[test]
    fn test_generate_standard_fixtures() {
        let fixtures = generate_standard_fixtures();
        assert!(fixtures.mp3_128.exists());
        assert!(fixtures.mp3_320.exists());
        assert!(fixtures.flac.exists());
        assert!(fixtures.wav.exists());
        assert!(fixtures.aac_256.exists());
        assert!(fixtures.ogg_192.exists());
    }

    #[test]
    fn test_create_test_album() {
        let album_path = create_test_album("Test Album", 3, "mp3");
        assert!(album_path.exists());
        assert!(album_path.join("01 - Track 1.mp3").exists());
        assert!(album_path.join("02 - Track 2.mp3").exists());
        assert!(album_path.join("03 - Track 3.mp3").exists());
    }
}
