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

pub use folder_state::{FolderConversionStatus, FolderId, calculate_folder_hash};
pub use scanning::{
    AudioFileInfo, MusicFolder, find_album_folders, format_duration, format_size, scan_music_folder,
};
pub use state::{
    AppSettings, BurnStage, ConversionState, DisplaySettings, ImportState, WindowState,
};
