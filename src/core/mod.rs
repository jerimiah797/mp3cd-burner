//! Core application logic and state
//!
//! This module contains:
//! - Application-wide state (settings, preferences)
//! - Actions that can be triggered from menus or UI
//! - Folder scanning and audio file discovery

mod scanning;
mod state;

pub use scanning::{
    format_duration, format_size, get_audio_files, scan_music_folder, total_duration, total_size,
    AudioFileInfo, MusicFolder,
};
pub use state::{AppSettings, BurnSettings};
