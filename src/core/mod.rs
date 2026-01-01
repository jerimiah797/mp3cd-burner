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

pub use scanning::{
    find_album_folders, format_duration, format_size, get_audio_files, scan_music_folder,
    AudioFileInfo, MusicFolder,
};
