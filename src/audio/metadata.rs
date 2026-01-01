use std::fs::{self, File};
use std::path::Path;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Extract album art from an audio file
pub fn get_album_art(path: &Path) -> Option<String> {
    println!("Attempting to extract album art from: {:?}", path);

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
    println!("Checking container metadata...");
    if let Some(metadata_rev) = probed.metadata.get() {
        println!("Found container metadata");
        if let Some(metadata_rev) = metadata_rev.current() {
            println!("Found current metadata revision with {} visuals", metadata_rev.visuals().len());
            for visual in metadata_rev.visuals() {
                println!("Found visual in container, data size: {} bytes", visual.data.len());
                if let Some(file_path) = save_album_art_to_temp(&visual.data, &visual.media_type) {
                    println!("Successfully saved album art to: {}", file_path);
                    return Some(file_path);
                }
            }
        }
    }

    // Check for embedded album art in format metadata
    println!("Checking format metadata...");
    let mut format = probed.format;
    if let Some(metadata_rev) = format.metadata().current() {
        println!("Found format metadata with {} visuals", metadata_rev.visuals().len());
        for visual in metadata_rev.visuals() {
            println!("Found visual in format metadata, data size: {} bytes", visual.data.len());
            if let Some(file_path) = save_album_art_to_temp(&visual.data, &visual.media_type) {
                println!("Successfully saved album art to: {}", file_path);
                return Some(file_path);
            }
        }
    }

    println!("No album art found");
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
        println!("Failed to create temp directory: {}", e);
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
        println!("Album art file already exists at: {:?}", file_path);
        return file_path.to_str().map(|s| s.to_string());
    }

    // Write the image data to file
    match File::create(&file_path) {
        Ok(mut file) => {
            if let Err(e) = file.write_all(data) {
                println!("Failed to write album art to file: {}", e);
                return None;
            }
            println!("Saved album art to: {:?}", file_path);
            file_path.to_str().map(|s| s.to_string())
        }
        Err(e) => {
            println!("Failed to create album art file: {}", e);
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
    let track = format.default_track()
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
    println!("File: {:?}, Codec string: {}", path.file_name(), codec_str);
    let codec = if codec_str.contains("MP3") || codec_str.contains("Mp3") {
        "mp3".to_string()
    } else if codec_str.contains("FLAC") || codec_str.contains("Flac") {
        "flac".to_string()
    } else if codec_str.contains("AAC") || codec_str.contains("Aac") || codec_str.contains("4100") {
        // CodecType(4100) is AAC
        "aac".to_string()
    } else if codec_str.contains("Vorbis") || codec_str.contains("OGG") {
        "ogg".to_string()
    } else if codec_str.contains("Opus") {
        "opus".to_string()
    } else if codec_str.contains("ALAC") || codec_str.contains("Alac") || codec_str.contains("4101") {
        // CodecType(4101) is ALAC
        "alac".to_string()
    } else if codec_str.contains("PCM") || codec_str.contains("Pcm") {
        // WAV or AIFF
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("wav")
            .to_lowercase()
    } else {
        // Fallback to file extension
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown")
            .to_lowercase()
    };

    // Determine if codec is lossy or lossless
    let is_lossy = matches!(codec.as_str(), "mp3" | "aac" | "ogg" | "opus" | "m4a");

    Ok((duration, bitrate, codec, is_lossy))
}
