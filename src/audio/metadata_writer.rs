//! Metadata writing for audio files
//!
//! This module provides functionality to write album metadata (album name, artist, year)
//! and individual track metadata (title, artist) to audio files. Used when users edit
//! metadata in the track editor.

use std::path::Path;

use lofty::{Accessor, Probe, Tag, TagExt, TaggedFileExt};

/// Album metadata to write to audio files
#[derive(Debug, Clone)]
pub struct WriteAlbumMetadata {
    pub album: Option<String>,
    pub artist: Option<String>,
    pub year: Option<String>,
}

/// Individual track metadata to write to audio files
#[derive(Debug, Clone)]
pub struct WriteTrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
}

/// Write album metadata to an audio file
///
/// Updates the primary tag of the file with the provided metadata.
/// Only fields that are Some will be updated.
pub fn write_album_metadata(path: &Path, metadata: &WriteAlbumMetadata) -> Result<(), String> {
    // Read the file
    let mut tagged_file = Probe::open(path)
        .map_err(|e| format!("Failed to open file: {}", e))?
        .read()
        .map_err(|e| format!("Failed to read file: {}", e))?;

    // Get or create the primary tag
    let tag = match tagged_file.primary_tag_mut() {
        Some(tag) => tag,
        None => {
            // No primary tag exists, try to create one
            let tag_type = tagged_file.primary_tag_type();
            tagged_file.insert_tag(Tag::new(tag_type));
            tagged_file
                .primary_tag_mut()
                .ok_or_else(|| "Failed to create tag".to_string())?
        }
    };

    // Update fields
    if let Some(album) = &metadata.album {
        tag.set_album(album.clone());
    }
    if let Some(artist) = &metadata.artist {
        tag.set_artist(artist.clone());
    }
    if let Some(year) = &metadata.year {
        if let Ok(y) = year.parse::<u32>() {
            tag.set_year(y);
        }
    }

    // Save the file
    tag.save_to_path(path)
        .map_err(|e| format!("Failed to save file: {}", e))?;

    Ok(())
}

/// Write individual track metadata to an audio file
///
/// Updates the primary tag of the file with the provided title and artist.
/// Only fields that are Some will be updated.
pub fn write_track_metadata(path: &Path, metadata: &WriteTrackMetadata) -> Result<(), String> {
    // Read the file
    let mut tagged_file = Probe::open(path)
        .map_err(|e| format!("Failed to open file: {}", e))?
        .read()
        .map_err(|e| format!("Failed to read file: {}", e))?;

    // Get or create the primary tag
    let tag = match tagged_file.primary_tag_mut() {
        Some(tag) => tag,
        None => {
            // No primary tag exists, try to create one
            let tag_type = tagged_file.primary_tag_type();
            tagged_file.insert_tag(Tag::new(tag_type));
            tagged_file
                .primary_tag_mut()
                .ok_or_else(|| "Failed to create tag".to_string())?
        }
    };

    // Update fields
    if let Some(title) = &metadata.title {
        tag.set_title(title.clone());
    }
    if let Some(artist) = &metadata.artist {
        tag.set_artist(artist.clone());
    }

    // Save the file
    tag.save_to_path(path)
        .map_err(|e| format!("Failed to save file: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_album_metadata_struct() {
        let metadata = WriteAlbumMetadata {
            album: Some("Test Album".to_string()),
            artist: Some("Test Artist".to_string()),
            year: Some("2024".to_string()),
        };

        assert_eq!(metadata.album, Some("Test Album".to_string()));
        assert_eq!(metadata.artist, Some("Test Artist".to_string()));
        assert_eq!(metadata.year, Some("2024".to_string()));
    }

    #[test]
    fn test_write_album_metadata_clone() {
        let metadata = WriteAlbumMetadata {
            album: Some("Album".to_string()),
            artist: None,
            year: Some("2023".to_string()),
        };
        let cloned = metadata.clone();
        assert_eq!(cloned.album, metadata.album);
        assert_eq!(cloned.artist, metadata.artist);
        assert_eq!(cloned.year, metadata.year);
    }

    #[test]
    fn test_write_album_metadata_nonexistent_file() {
        let metadata = WriteAlbumMetadata {
            album: Some("Test".to_string()),
            artist: None,
            year: None,
        };
        let result = write_album_metadata(Path::new("/nonexistent/file.mp3"), &metadata);
        assert!(result.is_err());
    }

    #[test]
    fn test_write_track_metadata_struct() {
        let metadata = WriteTrackMetadata {
            title: Some("Test Title".to_string()),
            artist: Some("Test Artist".to_string()),
        };

        assert_eq!(metadata.title, Some("Test Title".to_string()));
        assert_eq!(metadata.artist, Some("Test Artist".to_string()));
    }

    #[test]
    fn test_write_track_metadata_clone() {
        let metadata = WriteTrackMetadata {
            title: Some("Title".to_string()),
            artist: None,
        };
        let cloned = metadata.clone();
        assert_eq!(cloned.title, metadata.title);
        assert_eq!(cloned.artist, metadata.artist);
    }

    #[test]
    fn test_write_track_metadata_nonexistent_file() {
        let metadata = WriteTrackMetadata {
            title: Some("Test".to_string()),
            artist: None,
        };
        let result = write_track_metadata(Path::new("/nonexistent/file.mp3"), &metadata);
        assert!(result.is_err());
    }
}
