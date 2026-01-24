use std::fs::{self, File};
use std::path::Path;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTagKey};
use symphonia::core::probe::Hint;

/// Extract album art from an audio file
pub fn get_album_art(path: &Path) -> Option<String> {
    log::debug!("Attempting to extract album art from: {:?}", path);

    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(&ext.to_string_lossy());
    }

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let mut probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .ok()?;

    // Check for embedded album art in container metadata
    log::debug!("Checking container metadata...");
    if let Some(metadata_rev) = probed.metadata.get() {
        log::debug!("Found container metadata");
        if let Some(metadata_rev) = metadata_rev.current() {
            log::debug!(
                "Found current metadata revision with {} visuals",
                metadata_rev.visuals().len()
            );
            for visual in metadata_rev.visuals() {
                log::debug!(
                    "Found visual in container, data size: {} bytes",
                    visual.data.len()
                );
                if let Some(file_path) = save_album_art_to_temp(&visual.data, &visual.media_type) {
                    log::debug!("Successfully saved album art to: {}", file_path);
                    return Some(file_path);
                }
            }
        }
    }

    // Check for embedded album art in format metadata
    log::debug!("Checking format metadata...");
    let mut format = probed.format;
    if let Some(metadata_rev) = format.metadata().current() {
        log::debug!(
            "Found format metadata with {} visuals",
            metadata_rev.visuals().len()
        );
        for visual in metadata_rev.visuals() {
            log::debug!(
                "Found visual in format metadata, data size: {} bytes",
                visual.data.len()
            );
            if let Some(file_path) = save_album_art_to_temp(&visual.data, &visual.media_type) {
                log::debug!("Successfully saved album art to: {}", file_path);
                return Some(file_path);
            }
        }
    }

    log::debug!("No album art found");
    None
}

/// Save album art data to a temporary file
pub fn save_album_art_to_temp(data: &[u8], mime_type: &str) -> Option<String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::io::Write;

    let extension = match mime_type {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        _ => "jpg",
    };

    // Create temp directory if it doesn't exist
    let temp_dir = std::env::temp_dir().join("mp3cd_album_art");
    if let Err(e) = fs::create_dir_all(&temp_dir) {
        log::debug!("Failed to create temp directory: {}", e);
        return None;
    }

    // Generate a unique filename based on the data hash
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    let hash = hasher.finish();

    let filename = format!("album_art_{}.{}", hash, extension);
    let file_path = temp_dir.join(&filename);

    // If file already exists, just return the path
    if file_path.exists() {
        log::debug!("Album art file already exists at: {:?}", file_path);
        return file_path.to_str().map(|s| s.to_string());
    }

    // Write the image data to file
    match File::create(&file_path) {
        Ok(mut file) => {
            if let Err(e) = file.write_all(data) {
                log::debug!("Failed to write album art to file: {}", e);
                return None;
            }
            log::debug!("Saved album art to: {:?}", file_path);
            file_path.to_str().map(|s| s.to_string())
        }
        Err(e) => {
            log::debug!("Failed to create album art file: {}", e);
            None
        }
    }
}

/// Extract audio metadata: (duration, bitrate, codec, is_lossy)
pub fn get_audio_metadata(path: &Path) -> Result<(f64, u32, String, bool), String> {
    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(&ext.to_string_lossy());
    }

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| format!("Failed to probe audio format: {}", e))?;

    let format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| "No default track found".to_string())?;

    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100) as f64;
    let n_frames = track.codec_params.n_frames.unwrap_or(0);
    let duration = n_frames as f64 / sample_rate;

    // Calculate bitrate from file size and duration
    let file_size = fs::metadata(path)
        .map_err(|e| format!("Failed to get file metadata: {}", e))?
        .len();
    let bitrate = if duration > 0.0 {
        ((file_size * 8) as f64 / duration / 1000.0) as u32
    } else {
        0
    };

    // Detect codec from Symphonia's codec type or fall back to file extension
    let codec_str = format!("{:?}", track.codec_params.codec);
    log::debug!("File: {:?}, Codec string: {}", path.file_name(), codec_str);
    let codec = if codec_str.contains("MP3")
        || codec_str.contains("Mp3")
        || codec_str.contains("4099")
    {
        // CodecType(4099) is MP3
        "mp3".to_string()
    } else if codec_str.contains("FLAC") || codec_str.contains("Flac") || codec_str.contains("8192")
    {
        // CodecType(8192) is FLAC
        "flac".to_string()
    } else if codec_str.contains("AAC") || codec_str.contains("Aac") || codec_str.contains("4100") {
        // CodecType(4100) is AAC
        "aac".to_string()
    } else if codec_str.contains("Vorbis") || codec_str.contains("OGG") {
        "ogg".to_string()
    } else if codec_str.contains("Opus") || codec_str.contains("4101") {
        // CodecType(4101) is Opus
        "opus".to_string()
    } else if codec_str.contains("ALAC") || codec_str.contains("Alac") || codec_str.contains("8195")
    {
        // CodecType(8195) is ALAC
        "alac".to_string()
    } else if codec_str.contains("PCM") || codec_str.contains("Pcm") {
        // WAV or AIFF
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("wav")
            .to_lowercase()
    } else {
        // Fallback to file extension
        let ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown")
            .to_lowercase();
        // M4A is a container - use bitrate to distinguish AAC (lossy) from ALAC (lossless)
        if ext == "m4a" {
            if bitrate > 500 {
                "alac".to_string()
            } else {
                "aac".to_string()
            }
        } else {
            ext
        }
    };

    // Determine if codec is lossy or lossless
    let is_lossy = matches!(codec.as_str(), "mp3" | "aac" | "ogg" | "opus" | "webm");

    Ok((duration, bitrate, codec, is_lossy))
}

/// Album metadata extracted from audio files
#[derive(Debug, Clone, Default)]
pub struct AlbumMetadata {
    pub album: Option<String>,
    pub artist: Option<String>,
    pub year: Option<String>,
}

/// Track metadata extracted from audio files
#[derive(Debug, Clone, Default)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
}

/// Extract album metadata (album name, artist, year) from an audio file
pub fn get_album_metadata(path: &Path) -> AlbumMetadata {
    let mut metadata = AlbumMetadata::default();

    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return metadata,
    };

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(&ext.to_string_lossy());
    }

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let mut probed =
        match symphonia::default::get_probe().format(&hint, mss, &format_opts, &metadata_opts) {
            Ok(p) => p,
            Err(_) => return metadata,
        };

    // Helper to extract tags from a metadata revision
    let extract_tags = |metadata: &mut AlbumMetadata, tags: &[symphonia::core::meta::Tag]| {
        for tag in tags {
            match tag.std_key {
                Some(StandardTagKey::Album) => {
                    metadata.album = Some(tag.value.to_string());
                }
                Some(StandardTagKey::Artist) | Some(StandardTagKey::AlbumArtist) => {
                    // Prefer AlbumArtist, but use Artist if we don't have one yet
                    if metadata.artist.is_none()
                        || matches!(tag.std_key, Some(StandardTagKey::AlbumArtist))
                    {
                        metadata.artist = Some(tag.value.to_string());
                    }
                }
                Some(StandardTagKey::Date) | Some(StandardTagKey::OriginalDate) => {
                    // Extract just the year (first 4 chars) if it's a full date
                    let value = tag.value.to_string();
                    let year = if value.len() >= 4 {
                        value[..4].to_string()
                    } else {
                        value
                    };
                    // Prefer OriginalDate for year
                    if metadata.year.is_none()
                        || matches!(tag.std_key, Some(StandardTagKey::OriginalDate))
                    {
                        metadata.year = Some(year);
                    }
                }
                _ => {}
            }
        }
    };

    // Check container metadata first
    if let Some(metadata_rev) = probed.metadata.get()
        && let Some(current) = metadata_rev.current() {
            extract_tags(&mut metadata, current.tags());
        }

    // Also check format metadata (some formats store tags here)
    if let Some(current) = probed.format.metadata().current() {
        extract_tags(&mut metadata, current.tags());
    }

    metadata
}

/// Extract track metadata (title, artist) from an audio file
pub fn get_track_metadata(path: &Path) -> TrackMetadata {
    let mut metadata = TrackMetadata::default();

    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return metadata,
    };

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(&ext.to_string_lossy());
    }

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let mut probed =
        match symphonia::default::get_probe().format(&hint, mss, &format_opts, &metadata_opts) {
            Ok(p) => p,
            Err(_) => return metadata,
        };

    // Helper to extract tags from a metadata revision
    let extract_tags = |metadata: &mut TrackMetadata, tags: &[symphonia::core::meta::Tag]| {
        for tag in tags {
            match tag.std_key {
                Some(StandardTagKey::TrackTitle) => {
                    metadata.title = Some(tag.value.to_string());
                }
                Some(StandardTagKey::Artist) => {
                    if metadata.artist.is_none() {
                        metadata.artist = Some(tag.value.to_string());
                    }
                }
                _ => {}
            }
        }
    };

    // Check container metadata first
    if let Some(metadata_rev) = probed.metadata.get()
        && let Some(current) = metadata_rev.current()
    {
        extract_tags(&mut metadata, current.tags());
    }

    // Also check format metadata (some formats store tags here)
    if let Some(current) = probed.format.metadata().current() {
        extract_tags(&mut metadata, current.tags());
    }

    metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures;

    #[test]
    fn test_get_audio_metadata_mp3() {
        let path = test_fixtures::generate_audio_file("meta_test", "mp3", 3, Some(128));
        let result = get_audio_metadata(&path);
        assert!(result.is_ok(), "Should successfully read MP3 metadata");

        let (duration, bitrate, codec, is_lossy) = result.unwrap();
        assert!(duration > 2.5 && duration < 3.5, "Duration should be ~3 seconds, got {}", duration);
        assert!(bitrate > 100 && bitrate < 200, "Bitrate should be ~128 kbps, got {}", bitrate);
        assert_eq!(codec, "mp3");
        assert!(is_lossy, "MP3 should be lossy");
    }

    #[test]
    fn test_get_audio_metadata_flac() {
        let path = test_fixtures::generate_audio_file("meta_test", "flac", 3, None);
        let result = get_audio_metadata(&path);
        assert!(result.is_ok(), "Should successfully read FLAC metadata");

        let (duration, _bitrate, codec, is_lossy) = result.unwrap();
        assert!(duration > 2.5 && duration < 3.5, "Duration should be ~3 seconds, got {}", duration);
        assert_eq!(codec, "flac");
        assert!(!is_lossy, "FLAC should be lossless");
    }

    #[test]
    fn test_get_audio_metadata_wav() {
        let path = test_fixtures::generate_audio_file("meta_test", "wav", 3, None);
        let result = get_audio_metadata(&path);
        assert!(result.is_ok(), "Should successfully read WAV metadata");

        let (duration, _bitrate, codec, is_lossy) = result.unwrap();
        assert!(duration > 2.5 && duration < 3.5, "Duration should be ~3 seconds, got {}", duration);
        assert_eq!(codec, "wav");
        assert!(!is_lossy, "WAV should be lossless");
    }

    #[test]
    fn test_get_audio_metadata_aac() {
        let path = test_fixtures::generate_audio_file("meta_test", "aac", 3, Some(256));
        let result = get_audio_metadata(&path);
        assert!(result.is_ok(), "Should successfully read AAC metadata");

        let (duration, _bitrate, codec, is_lossy) = result.unwrap();
        assert!(duration > 2.5 && duration < 3.5, "Duration should be ~3 seconds, got {}", duration);
        assert_eq!(codec, "aac");
        assert!(is_lossy, "AAC should be lossy");
    }

    #[test]
    fn test_get_audio_metadata_ogg() {
        let path = test_fixtures::generate_audio_file("meta_test", "ogg", 3, Some(192));
        let result = get_audio_metadata(&path);
        assert!(result.is_ok(), "Should successfully read OGG metadata");

        let (duration, _bitrate, codec, is_lossy) = result.unwrap();
        assert!(duration > 2.5 && duration < 3.5, "Duration should be ~3 seconds, got {}", duration);
        assert_eq!(codec, "ogg");
        assert!(is_lossy, "OGG Vorbis should be lossy");
    }

    #[test]
    fn test_get_audio_metadata_nonexistent_file() {
        let path = Path::new("/nonexistent/file.mp3");
        let result = get_audio_metadata(path);
        assert!(result.is_err(), "Should fail for nonexistent file");
    }

    #[test]
    fn test_save_album_art_to_temp_jpeg() {
        let fake_jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
        let result = save_album_art_to_temp(&fake_jpeg, "image/jpeg");
        assert!(result.is_some(), "Should save JPEG album art");

        let path = result.unwrap();
        assert!(path.ends_with(".jpg"), "Should have .jpg extension");
        assert!(Path::new(&path).exists(), "File should exist");
    }

    #[test]
    fn test_save_album_art_to_temp_png() {
        let fake_png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let result = save_album_art_to_temp(&fake_png, "image/png");
        assert!(result.is_some(), "Should save PNG album art");

        let path = result.unwrap();
        assert!(path.ends_with(".png"), "Should have .png extension");
        assert!(Path::new(&path).exists(), "File should exist");
    }

    #[test]
    fn test_save_album_art_caches_identical_data() {
        let data = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];

        let path1 = save_album_art_to_temp(&data, "image/jpeg").unwrap();
        let path2 = save_album_art_to_temp(&data, "image/jpeg").unwrap();

        assert_eq!(path1, path2, "Same data should return same cached path");
    }

    #[test]
    fn test_get_album_art_returns_none_for_file_without_art() {
        let path = test_fixtures::generate_audio_file("no_art_test", "mp3", 2, Some(128));
        let result = get_album_art(&path);
        assert!(result.is_none(), "Generated file should not have album art");
    }

    #[test]
    fn test_album_metadata_default() {
        let metadata = AlbumMetadata::default();
        assert!(metadata.album.is_none());
        assert!(metadata.artist.is_none());
        assert!(metadata.year.is_none());
    }

    #[test]
    fn test_track_metadata_default() {
        let metadata = TrackMetadata::default();
        assert!(metadata.title.is_none());
        assert!(metadata.artist.is_none());
    }
}
