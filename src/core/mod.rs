//! Core application logic and state
//!
//! This module contains:
//! - Application-wide state (settings, preferences)
//! - Actions that can be triggered from menus or UI
//! - Folder scanning and audio file discovery
//! - Bitrate calculation for CD-fitting optimization

mod bitrate;
mod scanning;
mod state;

pub use bitrate::{
    calculate_estimated_output_size, calculate_optimal_bitrate, format_bitrate,
    get_encoding_decision, will_fit_on_cd, BitrateCalculation, EncodingDecision, MAX_BITRATE,
    MIN_BITRATE, TARGET_SIZE_BYTES,
};
pub use scanning::{
    format_duration, format_size, get_audio_files, scan_music_folder, total_duration, total_size,
    AudioFileInfo, MusicFolder,
};
pub use state::{AppSettings, BurnSettings};
