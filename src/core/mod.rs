//! Core application logic and state
//!
//! This module contains:
//! - Application-wide state (settings, preferences)
//! - Actions that can be triggered from menus or UI
//! - Folder scanning and audio file discovery
//! - Bitrate calculation for CD-fitting optimization
//! - Folder state tracking for background encoding

mod bitrate;
mod folder_state;
mod scanning;
mod state;

pub use folder_state::{
    calculate_folder_hash, FolderConversionStatus, FolderId, ListChangeType, ReencodeReason,
};
pub use scanning::{
    find_album_folders, format_duration, format_size, get_audio_files, scan_music_folder,
    AudioFileInfo, MusicFolder,
};
pub use state::AppSettings;
