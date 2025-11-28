// Audio module - contains audio detection, metadata, and conversion logic

pub mod detection;
pub mod metadata;
pub mod conversion;

pub use detection::is_audio_file;
pub use metadata::{get_album_art, get_audio_metadata, save_album_art_to_temp};
pub use conversion::{determine_encoding_strategy, EncodingStrategy};
