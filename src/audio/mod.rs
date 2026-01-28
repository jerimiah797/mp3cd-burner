// Audio module - contains audio detection, metadata, and conversion logic

pub mod conversion;
pub mod detection;
pub mod metadata;
pub mod metadata_writer;

pub use conversion::{EncodingStrategy, determine_encoding_strategy};
pub use detection::is_audio_file;
pub use metadata::{get_album_art, get_album_metadata, get_audio_metadata, get_track_metadata};
pub use metadata_writer::{WriteAlbumMetadata, WriteTrackMetadata, write_album_metadata, write_track_metadata};
