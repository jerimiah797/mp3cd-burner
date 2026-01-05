//! Application-wide actions
//!
//! Actions that can be triggered from menus or keyboard shortcuts.

use gpui::actions;
use std::path::PathBuf;
use std::sync::Mutex;

// Define actions for menu items
actions!(
    app,
    [
        Quit,
        About,
        OpenOutputDir,
        ToggleSimulateBurn,
        ToggleEmbedAlbumArt,
        OpenDisplaySettings,
        SetVolumeLabel,
        // Profile actions
        NewProfile,
        OpenProfile,
        SaveProfile,
    ]
);

/// Static storage for files opened via Finder/command line
///
/// When macOS opens a file with our app, the path is stored here
/// and FolderList polls for it during render.
pub static PENDING_OPEN_FILES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Add a path to be opened (called from on_open_urls callback)
pub fn push_pending_file(path: PathBuf) {
    if let Ok(mut paths) = PENDING_OPEN_FILES.lock() {
        paths.push(path);
    }
}

/// Take all pending paths (clears the queue)
pub fn take_pending_files() -> Vec<PathBuf> {
    if let Ok(mut paths) = PENDING_OPEN_FILES.lock() {
        std::mem::take(&mut *paths)
    } else {
        Vec::new()
    }
}
