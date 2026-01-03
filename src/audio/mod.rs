// Audio module - contains audio detection, metadata, and conversion logic

pub mod detection;
pub mod metadata;
pub mod conversion;

pub use detection::is_audio_file;
pub use metadata::{get_album_art, get_album_metadata, get_audio_metadata};
pub use conversion::{EncodingStrategy, determine_encoding_strategy};
