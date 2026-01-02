//! FolderList component - The main application view with folder list
//!
//! This is currently the root view of the application, containing:
//! - Header
//! - Folder list with drag-and-drop
//! - Status bar

use gpui::{div, prelude::*, rgb, AnyWindowHandle, AsyncApp, Context, ExternalPaths, FocusHandle, IntoElement, PromptLevel, Render, ScrollHandle, SharedString, Timer, WeakEntity, Window};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::folder_item::{render_folder_item, DraggedFolder, FolderItemProps};
use crate::audio::{determine_encoding_strategy, EncodingStrategy};
use crate::burning::{create_iso, burn_iso_with_cancel, check_cd_status, CdStatus};
use crate::conversion::{
    calculate_multipass_bitrate, ensure_output_dir, verify_ffmpeg,
    convert_files_parallel_with_callback, ConversionJob, ConversionProgress,
};
use crate::core::{find_album_folders, format_duration, get_audio_files, scan_music_folder, MusicFolder, AppSettings};
use crate::ui::Theme;

/// Calculate total size of files in a directory (recursive)
fn calculate_directory_size(path: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Current stage of the burn process
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurnStage {
    /// Converting audio files
    Converting,
    /// Creating ISO image
    CreatingIso,
    /// Waiting for user to insert a blank CD
    WaitingForCd,
    /// Detected an erasable disc (CD-RW) with data - waiting for user to confirm erase
    ErasableDiscDetected,
    /// Erasing CD-RW before burning
    Erasing,
    /// Burning ISO to CD
    Burning,
    /// Finishing up (closing session, verifying)
    Finishing,
    /// Process complete (success or simulated)
    Complete,
    /// Process was cancelled
    Cancelled,
}

impl BurnStage {
    #[allow(dead_code)]
    pub fn display_text(&self) -> &'static str {
        match self {
            BurnStage::Converting => "Converting...",
            BurnStage::CreatingIso => "Creating ISO...",
            BurnStage::WaitingForCd => "Insert blank CD",
            BurnStage::ErasableDiscDetected => "Erase disc?",
            BurnStage::Erasing => "Erasing...",
            BurnStage::Burning => "Burning...",
            BurnStage::Finishing => "Finishing...",
            BurnStage::Complete => "Complete!",
            BurnStage::Cancelled => "Cancelled",
        }
    }
}

/// The main folder list view
///
/// Handles:
/// - Displaying the list of folders
/// - External drag-drop from Finder (ExternalPaths)
/// - Internal drag-drop for reordering
/// - Empty state rendering
/// Shared state for tracking conversion progress across threads
#[derive(Clone)]
pub struct ConversionState {
    /// Whether conversion is currently running
    pub is_converting: Arc<AtomicBool>,
    /// Whether cancellation has been requested
    pub cancel_requested: Arc<AtomicBool>,
    /// Whether user has approved erasing a CD-RW
    pub erase_approved: Arc<AtomicBool>,
    /// Number of files completed
    pub completed: Arc<AtomicUsize>,
    /// Number of files failed
    pub failed: Arc<AtomicUsize>,
    /// Total number of files to convert
    pub total: Arc<AtomicUsize>,
    /// Current stage of the burn process
    pub stage: Arc<Mutex<BurnStage>>,
    /// Burn progress percentage (0-100, or -1 for indeterminate)
    pub burn_progress: Arc<std::sync::atomic::AtomicI32>,
    /// Path to the created ISO (for re-burning)
    pub iso_path: Arc<Mutex<Option<PathBuf>>>,
}

impl ConversionState {
    pub fn new() -> Self {
        Self {
            is_converting: Arc::new(AtomicBool::new(false)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            erase_approved: Arc::new(AtomicBool::new(false)),
            completed: Arc::new(AtomicUsize::new(0)),
            failed: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(0)),
            stage: Arc::new(Mutex::new(BurnStage::Converting)),
            burn_progress: Arc::new(std::sync::atomic::AtomicI32::new(-1)),
            iso_path: Arc::new(Mutex::new(None)),
        }
    }

    pub fn reset(&self, total: usize) {
        self.is_converting.store(true, Ordering::SeqCst);
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.erase_approved.store(false, Ordering::SeqCst);
        self.completed.store(0, Ordering::SeqCst);
        self.failed.store(0, Ordering::SeqCst);
        self.total.store(total, Ordering::SeqCst);
        *self.stage.lock().unwrap() = BurnStage::Converting;
        self.burn_progress.store(-1, Ordering::SeqCst);
        *self.iso_path.lock().unwrap() = None;
    }

    pub fn finish(&self) {
        self.is_converting.store(false, Ordering::SeqCst);
    }

    pub fn set_stage(&self, stage: BurnStage) {
        *self.stage.lock().unwrap() = stage;
    }

    pub fn get_stage(&self) -> BurnStage {
        *self.stage.lock().unwrap()
    }

    pub fn set_burn_progress(&self, progress: i32) {
        self.burn_progress.store(progress, Ordering::SeqCst);
    }

    pub fn get_burn_progress(&self) -> i32 {
        self.burn_progress.load(Ordering::SeqCst)
    }

    /// Request cancellation of the current conversion
    pub fn request_cancel(&self) {
        self.cancel_requested.store(true, Ordering::SeqCst);
    }

    /// Check if cancellation has been requested
    pub fn is_cancelled(&self) -> bool {
        self.cancel_requested.load(Ordering::SeqCst)
    }

    pub fn is_converting(&self) -> bool {
        self.is_converting.load(Ordering::SeqCst)
    }

    pub fn progress(&self) -> (usize, usize, usize) {
        (
            self.completed.load(Ordering::SeqCst),
            self.failed.load(Ordering::SeqCst),
            self.total.load(Ordering::SeqCst),
        )
    }
}

/// Shared state for tracking folder import progress across threads
#[derive(Clone)]
pub struct ImportState {
    /// Whether import is currently running
    pub is_importing: Arc<AtomicBool>,
    /// Number of folders scanned
    pub completed: Arc<AtomicUsize>,
    /// Total number of folders to scan
    pub total: Arc<AtomicUsize>,
    /// Scanned folders waiting to be added to the list
    pub scanned_folders: Arc<Mutex<Vec<MusicFolder>>>,
}

impl ImportState {
    pub fn new() -> Self {
        Self {
            is_importing: Arc::new(AtomicBool::new(false)),
            completed: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(0)),
            scanned_folders: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn reset(&self, total: usize) {
        self.is_importing.store(true, Ordering::SeqCst);
        self.completed.store(0, Ordering::SeqCst);
        self.total.store(total, Ordering::SeqCst);
        self.scanned_folders.lock().unwrap().clear();
    }

    pub fn finish(&self) {
        self.is_importing.store(false, Ordering::SeqCst);
    }

    pub fn is_importing(&self) -> bool {
        self.is_importing.load(Ordering::SeqCst)
    }

    pub fn progress(&self) -> (usize, usize) {
        (
            self.completed.load(Ordering::SeqCst),
            self.total.load(Ordering::SeqCst),
        )
    }

    /// Push a scanned folder to the queue
    pub fn push_folder(&self, folder: MusicFolder) {
        self.scanned_folders.lock().unwrap().push(folder);
        self.completed.fetch_add(1, Ordering::SeqCst);
    }

    /// Drain all scanned folders from the queue
    pub fn drain_folders(&self) -> Vec<MusicFolder> {
        let mut folders = self.scanned_folders.lock().unwrap();
        std::mem::take(&mut *folders)
    }
}

pub struct FolderList {
    /// The list of scanned music folders
    folders: Vec<MusicFolder>,
    /// Currently hovered drop target index (for visual feedback)
    drop_target_index: Option<usize>,
    /// Whether we've subscribed to appearance changes
    appearance_subscription_set: bool,
    /// Handle for scroll state
    scroll_handle: ScrollHandle,
    /// Conversion progress state
    conversion_state: ConversionState,
    /// Import progress state
    import_state: ImportState,
    /// Focus handle for receiving actions (None in tests)
    focus_handle: Option<FocusHandle>,
}

impl FolderList {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
            appearance_subscription_set: false,
            scroll_handle: ScrollHandle::new(),
            conversion_state: ConversionState::new(),
            import_state: ImportState::new(),
            focus_handle: Some(cx.focus_handle()),
        }
    }

    /// Create a new FolderList for testing (without GPUI context)
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
            appearance_subscription_set: false,
            scroll_handle: ScrollHandle::new(),
            conversion_state: ConversionState::new(),
            import_state: ImportState::new(),
            focus_handle: None,
        }
    }

    /// Returns the number of folders in the list
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.folders.len()
    }

    /// Returns true if the list is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }

    /// Returns an iterator over the folders
    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = &MusicFolder> {
        self.folders.iter()
    }

    /// Check if folder path is already in the list
    fn contains_path(&self, path: &PathBuf) -> bool {
        self.folders.iter().any(|f| f.path == *path)
    }

    /// Add folders from external drop (Finder)
    ///
    /// Scans each folder asynchronously in a background thread.
    /// Only adds directories that aren't already in the list.
    pub fn add_external_folders(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        // Don't start if already importing
        if self.import_state.is_importing() {
            println!("Import already in progress");
            return;
        }

        // Filter to only new directories (check on main thread before spawning)
        let new_paths: Vec<PathBuf> = paths
            .iter()
            .filter(|p| p.is_dir() && !self.contains_path(p))
            .cloned()
            .collect();

        if new_paths.is_empty() {
            return;
        }

        println!("Starting async import of {} folders", new_paths.len());

        // Reset import state (total will be updated after expansion)
        self.import_state.reset(new_paths.len());

        // Clone state for background thread
        let state = self.import_state.clone();

        // Spawn background thread for scanning
        std::thread::spawn(move || {
            // Expand each path into album folders (smart detection)
            let album_paths: Vec<PathBuf> = new_paths
                .iter()
                .flat_map(|p| find_album_folders(p))
                .collect();

            println!("Expanded to {} album folders", album_paths.len());

            // Reset state with actual count
            state.total.store(album_paths.len(), Ordering::SeqCst);

            for path in album_paths {
                println!("Scanning: {}", path.display());
                match scan_music_folder(&path) {
                    Ok(folder) => {
                        println!(
                            "Scanned folder: {} ({} files, {} bytes)",
                            folder.path.display(),
                            folder.file_count,
                            folder.total_size
                        );
                        state.push_folder(folder);
                    }
                    Err(e) => {
                        eprintln!("Failed to scan folder {}: {}", path.display(), e);
                        // Still increment completed so we know when done
                        state.completed.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            state.finish();
            println!("Import complete");
        });

        // Start polling for results
        Self::start_import_polling(self.import_state.clone(), cx);
    }

    /// Add a single folder to the list
    #[allow(dead_code)]
    pub fn add_folder(&mut self, path: PathBuf) {
        if path.is_dir() && !self.contains_path(&path) {
            if let Ok(folder) = scan_music_folder(&path) {
                self.folders.push(folder);
            }
        }
    }

    /// Remove a folder by index
    pub fn remove_folder(&mut self, index: usize) {
        if index < self.folders.len() {
            self.folders.remove(index);
        }
    }

    /// Move a folder from one index to another
    pub fn move_folder(&mut self, from: usize, to: usize) {
        if from < self.folders.len() && to <= self.folders.len() && from != to {
            let folder = self.folders.remove(from);
            let insert_at = if to > from { to - 1 } else { to };
            self.folders.insert(insert_at, folder);
        }
    }

    /// Clear all folders
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.folders.clear();
    }

    /// Get all folder paths (for saving profiles, etc.)
    #[allow(dead_code)]
    pub fn get_folder_paths(&self) -> Vec<PathBuf> {
        self.folders.iter().map(|f| f.path.clone()).collect()
    }

    /// Get all folders
    #[allow(dead_code)]
    pub fn get_folders(&self) -> &[MusicFolder] {
        &self.folders
    }

    /// Set folders from a saved profile (re-scans each folder)
    #[allow(dead_code)]
    pub fn set_folders(&mut self, paths: Vec<PathBuf>) {
        self.folders.clear();
        for path in paths {
            if let Ok(folder) = scan_music_folder(&path) {
                self.folders.push(folder);
            }
        }
    }

    /// Calculate total files across all folders
    pub fn total_files(&self) -> u32 {
        self.folders.iter().map(|f| f.file_count).sum()
    }

    /// Calculate total size across all folders
    pub fn total_size(&self) -> u64 {
        self.folders.iter().map(|f| f.total_size).sum()
    }

    /// Calculate total duration across all folders (in seconds)
    pub fn total_duration(&self) -> f64 {
        self.folders.iter().map(|f| f.total_duration).sum()
    }

    /// Calculate the optimal bitrate to fit on a 700MB CD
    ///
    /// Uses multi-pass-aware calculation:
    /// - MP3s are copied (exact size)
    /// - Lossy files transcoded at source bitrate
    /// - Lossless files get remaining space
    ///
    /// Returns the full estimate with bitrate and display logic
    pub fn calculated_bitrate_estimate(&self) -> Option<crate::conversion::MultipassEstimate> {
        if self.folders.is_empty() {
            return None;
        }

        // Collect all audio files from cached folder data
        let all_files: Vec<_> = self.folders
            .iter()
            .flat_map(|f| f.audio_files.iter().cloned())
            .collect();

        if all_files.is_empty() {
            return None;
        }

        // Use multi-pass-aware calculation
        Some(calculate_multipass_bitrate(&all_files))
    }

    /// Get the target bitrate for display (convenience wrapper)
    pub fn calculated_bitrate(&self) -> u32 {
        self.calculated_bitrate_estimate()
            .map(|e| e.target_bitrate)
            .unwrap_or(320)
    }

    /// Render the empty state drop zone
    fn render_empty_state(&self, theme: &Theme) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(theme.text_muted)
            .child(div().text_2xl().child("📂"))
            .child(div().text_lg().child("Drop music folders here"))
            .child(div().text_sm().child("or drag items to reorder"))
    }

    /// Render the populated folder list
    fn render_folder_items(&mut self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let drop_target = self.drop_target_index;
        let mut list = div().w_full().flex().flex_col().gap_2();

        for (index, folder) in self.folders.iter().enumerate() {
            let props = FolderItemProps {
                index,
                folder: folder.clone(),
                is_drop_target: drop_target == Some(index),
                theme: *theme,
            };

            let item = render_folder_item(
                props,
                cx,
                |view: &mut Self, from, to| {
                    view.move_folder(from, to);
                    view.drop_target_index = None;
                },
                |view: &mut Self, idx| {
                    view.remove_folder(idx);
                },
            );

            list = list.child(item);
        }

        list
    }
}

impl Render for FolderList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Subscribe to appearance changes (once)
        if !self.appearance_subscription_set {
            self.appearance_subscription_set = true;
            cx.observe_window_appearance(window, |_this, _window, cx| {
                cx.notify();
            })
            .detach();
        }

        // Get theme based on OS appearance
        let theme = Theme::from_appearance(window.appearance());
        let is_empty = self.folders.is_empty();

        // Build the folder list content
        let list_content = if is_empty {
            self.render_empty_state(&theme).into_any_element()
        } else {
            self.render_folder_items(&theme, cx).into_any_element()
        };

        // Capture listeners first (before borrowing for status bar)
        let on_external_drop = cx.listener(|this, paths: &ExternalPaths, _window, cx| {
            this.add_external_folders(paths.paths(), cx);
            this.drop_target_index = None;
        });

        let on_internal_drop = cx.listener(|this, dragged: &DraggedFolder, _window, _cx| {
            let target = this.folders.len();
            this.move_folder(dragged.index, target);
            this.drop_target_index = None;
        });

        // Build status bar after listeners
        let status_bar = self.render_status_bar(&theme, cx);

        // Build the base container
        let mut container = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg);

        // Track focus if we have a focus handle (not in tests)
        if let Some(ref focus_handle) = self.focus_handle {
            container = container.track_focus(focus_handle);
        }

        container
            // Handle external file drops on the entire window
            .on_drop(on_external_drop)
            // Style when dragging external files over window
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(0x3d3d3d)))
            // Main content area - folder list (scrollable)
            .child(
                div()
                    .id("folder-list-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll_handle)
                    .px_6() // Horizontal padding for breathing room
                    .py_2() // Vertical padding
                    // Handle drops on the list container
                    .on_drop(on_internal_drop)
                    .drag_over::<DraggedFolder>(|style, _, _, _| style.bg(rgb(0x3d3d3d)))
                    .child(list_content),
            )
            // Status bar at bottom
            .child(status_bar)
    }
}

impl FolderList {
    /// Render the status bar with detailed stats and action button
    fn render_status_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let total_files = self.total_files();
        let total_size = self.total_size();
        let total_duration = self.total_duration();
        let estimate = self.calculated_bitrate_estimate();
        let has_folders = !self.folders.is_empty();

        // Format bitrate display: show "--" if no lossless and no lossy capping needed
        let bitrate_display = match &estimate {
            Some(e) if e.should_show_bitrate() => format!("{} kbps", e.target_bitrate),
            _ => "--".to_string(),
        };

        let success_color = theme.success;
        let success_hover = theme.success_hover;
        let text_muted = theme.text_muted;
        let text_color = theme.text;
        let bg = theme.bg;

        // Format size in MB
        let size_mb = total_size as f64 / (1024.0 * 1024.0);

        div()
            .py_4()
            .px_6()
            .flex()
            .items_center()
            .justify_between()
            .bg(bg)
            .border_t_1()
            .border_color(theme.border)
            .text_sm()
            // Left side: stats in rows
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_color(text_muted)
                    // Row 1: Files and Duration
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child("Files:")
                                    .child(
                                        div()
                                            .text_color(text_color)
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .child(format!("{}", total_files)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child("Duration:")
                                    .child(
                                        div()
                                            .text_color(text_color)
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .child(format_duration(total_duration)),
                                    ),
                            ),
                    )
                    // Row 2: Size and Target
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child("Size:")
                                    .child(
                                        div()
                                            .text_color(text_color)
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .child(format!("{:.2} MB", size_mb)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child("Target:")
                                    .child(
                                        div()
                                            .text_color(text_color)
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .child("700 MB"),
                                    ),
                            ),
                    )
                    // Row 3: Bitrate (in accent/success color) and CD-RW indicator
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child("Bitrate:")
                                    .child(
                                        div()
                                            .text_color(success_color)
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .child(bitrate_display.clone()),
                                    ),
                            )
                            // CD-RW indicator (only show when erasable disc detected)
                            .when(
                                self.conversion_state.is_converting()
                                    && self.conversion_state.get_stage() == BurnStage::ErasableDiscDetected,
                                |el| {
                                    el.child(
                                        div()
                                            .text_color(theme.danger)
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .child("CD-RW"),
                                    )
                                }
                            ),
                    ),
            )
            // Right side: Convert & Burn button / Progress display
            .child({
                let is_converting = self.conversion_state.is_converting();
                let is_importing = self.import_state.is_importing();
                let (completed, failed, total) = self.conversion_state.progress();
                let (import_completed, import_total) = self.import_state.progress();

                if is_importing {
                    // Show import progress
                    let progress_fraction = if import_total > 0 {
                        import_completed as f32 / import_total as f32
                    } else {
                        0.0
                    };

                    div()
                        .id(SharedString::from("import-progress"))
                        .w(gpui::px(140.0))
                        .h(gpui::px(70.0))
                        .rounded_md()
                        .border_1()
                        .border_color(theme.accent)
                        .overflow_hidden()
                        .relative()
                        // Background progress fill
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(gpui::relative(progress_fraction))
                                .bg(theme.accent)
                        )
                        // Text overlay
                        .child(
                            div()
                                .size_full()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .relative()
                                .child(
                                    div()
                                        .text_lg()
                                        .text_color(gpui::white())
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child(format!("{}/{}", import_completed, import_total))
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(gpui::white())
                                        .child("Importing...")
                                )
                        )
                } else if is_converting {
                    // Show progress bar during conversion/burn with cancel button
                    let stage = self.conversion_state.get_stage();
                    let burn_progress = self.conversion_state.get_burn_progress();
                    let is_cancelled = self.conversion_state.is_cancelled();
                    let cancel_color = theme.danger;

                    // Calculate progress based on current stage
                    let (progress_fraction, progress_text, stage_text) = match stage {
                        BurnStage::Converting => {
                            let frac = if total > 0 {
                                (completed + failed) as f32 / total as f32
                            } else {
                                0.0
                            };
                            (frac, format!("{}/{}", completed + failed, total), "Converting...")
                        }
                        BurnStage::CreatingIso => {
                            (1.0, "".to_string(), "Creating ISO...")
                        }
                        BurnStage::WaitingForCd => {
                            (1.0, "".to_string(), "Insert blank CD")
                        }
                        BurnStage::ErasableDiscDetected => {
                            (1.0, "".to_string(), "CD-RW detected")
                        }
                        BurnStage::Erasing => {
                            let frac = if burn_progress >= 0 {
                                burn_progress as f32 / 100.0
                            } else {
                                0.0
                            };
                            let text = if burn_progress >= 0 {
                                format!("{}%", burn_progress)
                            } else {
                                "".to_string()
                            };
                            (frac, text, "Erasing...")
                        }
                        BurnStage::Burning => {
                            let frac = if burn_progress >= 0 {
                                burn_progress as f32 / 100.0
                            } else {
                                0.0 // Start at 0 until we get real progress
                            };
                            let text = if burn_progress >= 0 {
                                format!("{}%", burn_progress)
                            } else {
                                "".to_string()
                            };
                            (frac, text, "Burning...")
                        }
                        BurnStage::Finishing => {
                            (1.0, "".to_string(), "Finishing...")
                        }
                        BurnStage::Complete => {
                            (1.0, "✓".to_string(), "Complete!")
                        }
                        BurnStage::Cancelled => {
                            (0.0, "".to_string(), "Cancelled")
                        }
                    };

                    let stage_color = match stage {
                        BurnStage::Cancelled => cancel_color,
                        BurnStage::Complete => success_color,
                        _ if is_cancelled => cancel_color,
                        _ => success_color,
                    };

                    div()
                        .id(SharedString::from("convert-progress-container"))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .items_center()
                        // Progress display (hide when waiting for user to approve erase)
                        .when(stage != BurnStage::ErasableDiscDetected, |el| {
                            el.child(
                                div()
                                    .w(gpui::px(140.0))
                                    .h(gpui::px(50.0))
                                    .rounded_md()
                                    .border_1()
                                    .border_color(stage_color)
                                    .overflow_hidden()
                                    .relative()
                                    // Background progress fill
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .top_0()
                                            .h_full()
                                            .w(gpui::relative(progress_fraction))
                                            .bg(stage_color)
                                    )
                                    // Text overlay
                                    .child(
                                        div()
                                            .size_full()
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .justify_center()
                                            .relative()
                                            .when(!progress_text.is_empty(), |el| {
                                                el.child(
                                                    div()
                                                        .text_lg()
                                                        .text_color(gpui::white())
                                                        .font_weight(gpui::FontWeight::BOLD)
                                                        .child(progress_text.clone())
                                                )
                                            })
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(gpui::white())
                                                    .child(if is_cancelled && stage != BurnStage::Cancelled {
                                                        "Cancelling..."
                                                    } else {
                                                        stage_text
                                                    })
                                            )
                                    )
                            )
                        })
                        // Erase & Burn button (only show when erasable disc detected)
                        .when(stage == BurnStage::ErasableDiscDetected, |el| {
                            el.child(
                                div()
                                    .id(SharedString::from("erase-burn-btn"))
                                    .px_4()
                                    .py_1()
                                    .bg(success_color)
                                    .text_color(gpui::white())
                                    .text_sm()
                                    .rounded_md()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(gpui::rgb(0x16a34a))) // darker green on hover
                                    .on_click(cx.listener(|this, _event, _window, _cx| {
                                        println!("Erase & Burn clicked");
                                        this.conversion_state.erase_approved.store(true, Ordering::SeqCst);
                                    }))
                                    .child("Erase & Burn")
                            )
                        })
                        // Cancel button (only show during active stages)
                        .when(stage != BurnStage::Complete && stage != BurnStage::Cancelled, |el| {
                            el.child(
                                div()
                                    .id(SharedString::from("cancel-btn"))
                                    .px_4()
                                    .py_1()
                                    .bg(if is_cancelled { text_muted } else { cancel_color })
                                    .text_color(gpui::white())
                                    .text_sm()
                                    .rounded_md()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_center()
                                    .when(!is_cancelled, |el| {
                                        el.cursor_pointer()
                                            .hover(|s| s.bg(gpui::rgb(0xdc2626))) // darker red on hover
                                            .on_click(cx.listener(|this, _event, _window, _cx| {
                                                println!("Cancel button clicked");
                                                this.conversion_state.request_cancel();
                                            }))
                                    })
                                    .child(if is_cancelled { "Cancelling" } else { "Cancel" })
                            )
                        })
                } else {
                    // Normal Convert & Burn button
                    div()
                        .id(SharedString::from("convert-burn-btn"))
                        .px(gpui::px(55.0))  // ~70% wider than original
                        .h(gpui::px(70.0))   // Match status text block height
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if has_folders { success_color } else { text_muted })
                        .text_color(gpui::white())
                        .text_lg()
                        .rounded_md()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_center()
                        .when(has_folders, |el| {
                            el.cursor_pointer().hover(|s| s.bg(success_hover))
                        })
                        .on_click(cx.listener(move |this, _event, window, cx| {
                            if has_folders {
                                println!("Convert & Burn clicked!");
                                this.run_conversion(window, cx);
                            }
                        }))
                        .child("Convert\n& Burn")
                }
            })
    }

    /// Cancel any ongoing conversion
    ///
    /// This sets the cancellation flag which will stop new files from being processed.
    /// Files that are currently being converted will finish, but no new files will start.
    /// Returns true if there was a conversion to cancel.
    #[allow(dead_code)]
    pub fn cancel_conversion(&mut self) -> bool {
        if self.conversion_state.is_converting() {
            println!("Cancelling conversion...");
            self.conversion_state.request_cancel();
            true
        } else {
            false
        }
    }

    /// Run the conversion process for all folders (async, in background thread)
    fn run_conversion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Don't start if already converting
        if self.conversion_state.is_converting() {
            println!("Conversion already in progress");
            return;
        }

        println!("Starting conversion...");

        // Verify ffmpeg is available
        let ffmpeg_path = match verify_ffmpeg() {
            Ok(path) => {
                println!("Using ffmpeg at: {:?}", path);
                path
            }
            Err(e) => {
                eprintln!("FFmpeg not found: {}", e);
                return;
            }
        };

        // Create output directory
        let output_dir = match ensure_output_dir() {
            Ok(dir) => {
                println!("Output directory: {:?}", dir);
                dir
            }
            Err(e) => {
                eprintln!("Failed to create output directory: {}", e);
                return;
            }
        };

        // Calculate initial target bitrate
        let initial_bitrate = self.calculated_bitrate();
        println!("Initial calculated bitrate: {} kbps", initial_bitrate);

        // First pass: collect all audio files for optimization
        let mut all_audio_files = Vec::new();
        let mut folder_info: Vec<(usize, String, std::path::PathBuf)> = Vec::new();

        for (folder_idx, folder) in self.folders.iter().enumerate() {
            let folder_name = folder.path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown");
            let album_dir_name = format!("{:02}-{}", folder_idx + 1, folder_name);
            let album_output_dir = output_dir.join(&album_dir_name);

            let audio_files = match get_audio_files(&folder.path) {
                Ok(files) => files,
                Err(e) => {
                    eprintln!("Failed to get audio files from {}: {}", folder.path.display(), e);
                    continue;
                }
            };

            for audio_file in audio_files {
                folder_info.push((all_audio_files.len(), album_dir_name.clone(), album_output_dir.clone()));
                all_audio_files.push(audio_file);
            }
        }

        if all_audio_files.is_empty() {
            println!("No audio files to convert");
            return;
        }

        // Multi-pass approach: partition files by encoding strategy
        // - Pass 1: Copy/strip MP3s (exact size known after)
        // - Pass 2: Transcode lossy at source bitrate (size known after)
        // - Pass 3: Transcode lossless at calculated bitrate (fills remaining space)

        let mut copy_jobs: Vec<ConversionJob> = Vec::new();
        let mut lossy_jobs: Vec<ConversionJob> = Vec::new();
        // For lossless, we store path info and duration - bitrate calculated after passes 1+2
        let mut lossless_info: Vec<(PathBuf, PathBuf, f64)> = Vec::new(); // (input, output, duration)

        for (idx, audio_file) in all_audio_files.into_iter().enumerate() {
            let (_, _, ref album_output_dir) = folder_info[idx];

            let file_stem = audio_file.path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("output");
            let output_path = album_output_dir.join(format!("{}.mp3", file_stem));

            // Determine strategy using initial bitrate (just for categorization)
            let strategy = determine_encoding_strategy(
                &audio_file.codec,
                audio_file.bitrate,
                initial_bitrate,
                audio_file.is_lossy,
                false,
                false,
            );

            match &strategy {
                EncodingStrategy::Copy | EncodingStrategy::CopyWithoutArt => {
                    copy_jobs.push(ConversionJob {
                        input_path: audio_file.path,
                        output_path,
                        strategy,
                    });
                }
                EncodingStrategy::ConvertAtSourceBitrate(_) => {
                    lossy_jobs.push(ConversionJob {
                        input_path: audio_file.path,
                        output_path,
                        strategy,
                    });
                }
                EncodingStrategy::ConvertAtTargetBitrate(_) => {
                    // Store for later - bitrate will be calculated after passes 1+2
                    lossless_info.push((audio_file.path, output_path, audio_file.duration));
                }
            }
        }

        let total_jobs = copy_jobs.len() + lossy_jobs.len() + lossless_info.len();
        if total_jobs == 0 {
            println!("No files to convert");
            return;
        }

        println!(
            "Multi-pass conversion: {} copy, {} lossy transcode, {} lossless (bitrate TBD)",
            copy_jobs.len(),
            lossy_jobs.len(),
            lossless_info.len()
        );

        // Reset conversion state
        self.conversion_state.reset(total_jobs);

        // Clone state for the background thread
        let state = self.conversion_state.clone();
        let cancel_token = self.conversion_state.cancel_requested.clone();
        let output_dir_clone = output_dir.clone();
        let simulate_burn = cx.global::<AppSettings>().simulate_burn;

        // Spawn background thread with tokio runtime
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

            rt.block_on(async {
                let progress = Arc::new(ConversionProgress::new(total_jobs));
                let mut was_cancelled = false;

                // === PASS 1: Copy MP3s ===
                let copy_count = copy_jobs.len();
                if !copy_jobs.is_empty() && !was_cancelled {
                    println!("\n=== Pass 1: Copying {} MP3 files ===", copy_count);
                    let progress_for_callback = progress.clone();
                    let state_for_callback = state.clone();

                    let (_completed, failed, cancelled) = convert_files_parallel_with_callback(
                        ffmpeg_path.clone(),
                        copy_jobs,
                        progress.clone(),
                        cancel_token.clone(),
                        move || {
                            let completed = progress_for_callback.completed_count();
                            let failed = progress_for_callback.failed_count();
                            state_for_callback.completed.store(completed, Ordering::SeqCst);
                            state_for_callback.failed.store(failed, Ordering::SeqCst);
                        },
                    ).await;

                    was_cancelled = cancelled;
                    println!("Pass 1 complete: {} copied, {} failed", copy_count - failed, failed);
                }

                // === PASS 2: Transcode lossy files ===
                let lossy_count = lossy_jobs.len();
                if !lossy_jobs.is_empty() && !was_cancelled {
                    println!("\n=== Pass 2: Transcoding {} lossy files ===", lossy_count);
                    let progress_for_callback = progress.clone();
                    let state_for_callback = state.clone();

                    let (_completed, failed, cancelled) = convert_files_parallel_with_callback(
                        ffmpeg_path.clone(),
                        lossy_jobs,
                        progress.clone(),
                        cancel_token.clone(),
                        move || {
                            let completed = progress_for_callback.completed_count();
                            let failed = progress_for_callback.failed_count();
                            state_for_callback.completed.store(completed, Ordering::SeqCst);
                            state_for_callback.failed.store(failed, Ordering::SeqCst);
                        },
                    ).await;

                    was_cancelled = cancelled;
                    println!("Pass 2 complete: {} transcoded, {} failed", lossy_count - failed, failed);
                }

                // === Calculate remaining space for lossless ===
                if !lossless_info.is_empty() && !was_cancelled {
                    // Measure actual output size after passes 1+2
                    let current_size = calculate_directory_size(&output_dir_clone);
                    let cd_capacity: u64 = 685 * 1024 * 1024;
                    let remaining_space = cd_capacity.saturating_sub(current_size);

                    // Calculate total lossless duration
                    let total_lossless_duration: f64 = lossless_info.iter().map(|(_, _, d)| d).sum();

                    // Calculate optimal bitrate: remaining_bytes * 8 / duration / 1000 = kbps
                    // Using CBR mode for lossless, so output size is predictable
                    // Subtract 2kbps for MP3 header/framing overhead
                    let optimal_bitrate = if total_lossless_duration > 0.0 {
                        let raw_bitrate = remaining_space as f64 * 8.0 / total_lossless_duration / 1000.0;
                        let adjusted_bitrate = (raw_bitrate - 2.0) as u32; // small margin for overhead
                        adjusted_bitrate.clamp(64, 320)
                    } else {
                        256 // fallback
                    };

                    println!(
                        "\n=== Pass 3: Transcoding {} lossless files ===",
                        lossless_info.len()
                    );
                    println!(
                        "Current output: {:.1} MB, Remaining: {:.1} MB, Lossless duration: {:.0}s",
                        current_size as f64 / 1024.0 / 1024.0,
                        remaining_space as f64 / 1024.0 / 1024.0,
                        total_lossless_duration
                    );
                    println!("Calculated optimal bitrate: {} kbps", optimal_bitrate);

                    // Build lossless jobs with calculated bitrate
                    let lossless_jobs: Vec<ConversionJob> = lossless_info
                        .into_iter()
                        .map(|(input_path, output_path, _)| ConversionJob {
                            input_path,
                            output_path,
                            strategy: EncodingStrategy::ConvertAtTargetBitrate(optimal_bitrate),
                        })
                        .collect();

                    let lossless_count = lossless_jobs.len();
                    let progress_for_callback = progress.clone();
                    let state_for_callback = state.clone();

                    let (_completed, failed, cancelled) = convert_files_parallel_with_callback(
                        ffmpeg_path,
                        lossless_jobs,
                        progress.clone(),
                        cancel_token.clone(),
                        move || {
                            let completed = progress_for_callback.completed_count();
                            let failed = progress_for_callback.failed_count();
                            state_for_callback.completed.store(completed, Ordering::SeqCst);
                            state_for_callback.failed.store(failed, Ordering::SeqCst);
                        },
                    ).await;

                    was_cancelled = cancelled;
                    println!("Pass 3 complete: {} transcoded, {} failed", lossless_count - failed, failed);
                }

                // Final output size
                let final_size = calculate_directory_size(&output_dir_clone);
                let utilization = final_size as f64 / (685.0 * 1024.0 * 1024.0) * 100.0;

                // Get final counts from progress tracker (cumulative across all passes)
                let total_completed = progress.completed_count();
                let total_failed = progress.failed_count();

                if was_cancelled {
                    println!(
                        "\nConversion CANCELLED: {} converted, {} failed before cancel",
                        total_completed, total_failed
                    );
                    state.set_stage(BurnStage::Cancelled);
                    state.finish();
                    return;
                }

                println!(
                    "\nConversion complete: {} converted, {} failed",
                    total_completed, total_failed
                );
                println!(
                    "Final output: {:.1} MB ({:.1}% of CD capacity)",
                    final_size as f64 / 1024.0 / 1024.0,
                    utilization
                );

                // === ISO CREATION ===
                state.set_stage(BurnStage::CreatingIso);
                println!("\n=== Creating ISO image ===");

                // Generate volume label from folder names (first few albums)
                let volume_label = "MP3CD".to_string(); // TODO: generate from folder names

                match create_iso(&output_dir_clone, &volume_label) {
                    Ok(result) => {
                        println!("ISO created at: {}", result.iso_path.display());
                        *state.iso_path.lock().unwrap() = Some(result.iso_path.clone());

                        if simulate_burn {
                            // Simulated mode - skip actual burning
                            println!("\n=== SIMULATED BURN ===");
                            println!("Would burn ISO: {}", result.iso_path.display());
                            state.set_stage(BurnStage::Complete);
                            state.finish();
                        } else {
                            // Real mode - check for CD and burn
                            state.set_stage(BurnStage::WaitingForCd);
                            println!("\n=== Waiting for blank CD ===");

                            // Poll for CD insertion (with timeout)
                            let mut erase_first = false;
                            let mut cd_ready = false;

                            for _ in 0..120 {
                                // Wait up to 120 seconds (longer to allow for erase prompt)
                                if cancel_token.load(Ordering::SeqCst) {
                                    println!("Burn cancelled while waiting for CD");
                                    state.set_stage(BurnStage::Cancelled);
                                    state.finish();
                                    return;
                                }

                                match check_cd_status() {
                                    Ok(CdStatus::Blank) => {
                                        println!("Blank CD detected");
                                        cd_ready = true;
                                        break;
                                    }
                                    Ok(CdStatus::ErasableWithData) => {
                                        // CD-RW with data - prompt user to erase
                                        println!("Erasable disc (CD-RW) with data detected");
                                        state.set_stage(BurnStage::ErasableDiscDetected);

                                        // Wait for user to approve erase or cancel
                                        loop {
                                            if cancel_token.load(Ordering::SeqCst) {
                                                println!("Burn cancelled");
                                                state.set_stage(BurnStage::Cancelled);
                                                state.finish();
                                                return;
                                            }
                                            if state.erase_approved.load(Ordering::SeqCst) {
                                                println!("User approved erase - will erase and burn");
                                                erase_first = true;
                                                cd_ready = true;
                                                break;
                                            }
                                            std::thread::sleep(std::time::Duration::from_millis(100));
                                        }
                                        break;
                                    }
                                    Ok(CdStatus::NonErasable) => {
                                        // Non-erasable disc with data - wait for different disc
                                        println!("Non-erasable disc detected - please insert a blank disc");
                                        std::thread::sleep(std::time::Duration::from_secs(2));
                                    }
                                    Ok(CdStatus::NoDisc) => {
                                        std::thread::sleep(std::time::Duration::from_secs(1));
                                    }
                                    Err(e) => {
                                        eprintln!("Error checking CD: {}", e);
                                        std::thread::sleep(std::time::Duration::from_secs(1));
                                    }
                                }
                            }

                            if !cd_ready {
                                println!("No usable CD found after timeout");
                                state.set_stage(BurnStage::Complete);
                                state.finish();
                                return;
                            }

                            // === BURN CD ===
                            if erase_first {
                                state.set_stage(BurnStage::Erasing);
                                println!("\n=== Erasing and Burning CD ===");
                            } else {
                                state.set_stage(BurnStage::Burning);
                                println!("\n=== Burning CD ===");
                            }

                            // Track progress to detect phase transition (erase -> burn)
                            let state_for_progress = state.clone();
                            let last_progress = Arc::new(std::sync::atomic::AtomicI32::new(-1));
                            let last_progress_clone = last_progress.clone();
                            let is_erasing = erase_first;

                            let progress_callback = Box::new(move |progress: i32| {
                                let current_stage = state_for_progress.get_stage();
                                let prev = last_progress_clone.load(Ordering::SeqCst);

                                // Handle -1 (indeterminate) values
                                if progress < 0 {
                                    // If we were at high progress (>=95) in Burning stage, switch to Finishing
                                    if prev >= 95 && current_stage == BurnStage::Burning {
                                        state_for_progress.set_stage(BurnStage::Finishing);
                                    }
                                    return;
                                }

                                // Store current progress for next comparison
                                last_progress_clone.store(progress, Ordering::SeqCst);

                                // Detect phase transition: progress was high (>50) and now low (<20)
                                // This indicates erase completed and burn started
                                if is_erasing && prev > 50 && progress < 20 && current_stage == BurnStage::Erasing {
                                    state_for_progress.set_stage(BurnStage::Burning);
                                }

                                state_for_progress.set_burn_progress(progress);
                            });

                            // Pass cancel token and erase flag
                            match burn_iso_with_cancel(&result.iso_path, Some(progress_callback), Some(cancel_token.clone()), erase_first) {
                                Ok(()) => {
                                    println!("CD burned successfully!");
                                    state.set_stage(BurnStage::Complete);
                                }
                                Err(e) if e.contains("cancelled") => {
                                    println!("Burn was cancelled");
                                    state.set_stage(BurnStage::Cancelled);
                                }
                                Err(e) => {
                                    eprintln!("Burn failed: {}", e);
                                    state.set_stage(BurnStage::Complete); // Still mark complete
                                }
                            }
                            state.finish();
                        }
                    }
                    Err(e) => {
                        eprintln!("ISO creation failed: {}", e);
                        state.set_stage(BurnStage::Complete);
                        state.finish();
                    }
                }
            });
        });

        // Start polling for progress updates (pass window handle for success dialog)
        let window_handle = window.window_handle();
        Self::start_progress_polling(self.conversion_state.clone(), window_handle, cx);

        println!("Multi-pass conversion started ({} files)", total_jobs);
        cx.notify();
    }

    /// Start a polling loop that updates the UI periodically during conversion
    fn start_progress_polling(
        state: ConversionState,
        window_handle: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        // state is already cloned - no need to read entity

        // Clone in sync part BEFORE the async block - key to avoiding lifetime issues
        cx.spawn(move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone(); // Clone here, in sync context
            async move {
                // Poll until conversion finishes
                loop {
                    // Clone BEFORE the await
                    let cx_for_after_await = async_cx.clone();

                    // Wait 50ms between UI updates for smooth progress
                    Timer::after(std::time::Duration::from_millis(50)).await;

                    // Check if we should continue
                    if !state.is_converting() {
                        break;
                    }

                    // Refresh all windows to show updated progress
                    let _ = cx_for_after_await.refresh();

                    // Use this clone for next iteration
                    async_cx = cx_for_after_await;
                }

                // Final refresh to show completion state
                let _ = async_cx.refresh();

                // Show success dialog if completed (not cancelled)
                let final_stage = state.get_stage();
                if final_stage == BurnStage::Complete {
                    let _ = async_cx.update_window(window_handle, |_, window, cx| {
                        let _ = window.prompt(
                            PromptLevel::Info,
                            "Burn Complete",
                            Some("The CD has been burned successfully."),
                            &["OK"],
                            cx,
                        );
                    });
                }
            }
        }).detach();
    }

    /// Start a polling loop that drains imported folders and updates the UI
    fn start_import_polling(state: ImportState, cx: &mut Context<Self>) {
        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                loop {
                    let cx_for_after_await = async_cx.clone();

                    // Wait 50ms between updates
                    Timer::after(std::time::Duration::from_millis(50)).await;

                    // Drain any scanned folders and add to the list
                    let folders = state.drain_folders();
                    if !folders.is_empty() {
                        let _ = this.update(&mut async_cx, |this, _cx| {
                            for folder in folders {
                                this.folders.push(folder);
                            }
                        });
                    }

                    // Check if we should continue
                    if !state.is_importing() {
                        break;
                    }

                    // Refresh UI
                    let _ = cx_for_after_await.refresh();

                    async_cx = cx_for_after_await;
                }

                // Final drain and refresh
                let folders = state.drain_folders();
                if !folders.is_empty() {
                    let _ = this.update(&mut async_cx, |this, _cx| {
                        for folder in folders {
                            this.folders.push(folder);
                        }
                    });
                }
                let _ = async_cx.refresh();
            }
        }).detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Helper to create a test MusicFolder
    fn test_folder(path: &str) -> MusicFolder {
        MusicFolder {
            path: PathBuf::from(path),
            file_count: 10,
            total_size: 50_000_000,
            total_duration: 2400.0, // 40 minutes
            album_art: None,
            audio_files: Vec::new(),
        }
    }

    #[test]
    fn test_folder_list_new() {
        let list = FolderList::new_for_test();
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_add_folder() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/folder1"));
        list.folders.push(test_folder("/test/folder2"));

        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_remove_folder() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/folder1"));
        list.folders.push(test_folder("/test/folder2"));

        list.remove_folder(0);

        assert_eq!(list.len(), 1);
        assert_eq!(list.folders[0].path, PathBuf::from("/test/folder2"));
    }

    #[test]
    fn test_move_folder_forward() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/a"));
        list.folders.push(test_folder("/test/b"));
        list.folders.push(test_folder("/test/c"));

        // Move "a" to position 2 (after "b")
        list.move_folder(0, 2);

        assert_eq!(list.folders[0].path, PathBuf::from("/test/b"));
        assert_eq!(list.folders[1].path, PathBuf::from("/test/a"));
        assert_eq!(list.folders[2].path, PathBuf::from("/test/c"));
    }

    #[test]
    fn test_move_folder_backward() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/a"));
        list.folders.push(test_folder("/test/b"));
        list.folders.push(test_folder("/test/c"));

        // Move "c" to position 0 (before "a")
        list.move_folder(2, 0);

        assert_eq!(list.folders[0].path, PathBuf::from("/test/c"));
        assert_eq!(list.folders[1].path, PathBuf::from("/test/a"));
        assert_eq!(list.folders[2].path, PathBuf::from("/test/b"));
    }

    #[test]
    fn test_clear() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/folder1"));
        list.folders.push(test_folder("/test/folder2"));

        list.clear();

        assert!(list.is_empty());
    }

    #[test]
    fn test_total_files() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/folder1")); // 10 files
        list.folders.push(test_folder("/test/folder2")); // 10 files

        assert_eq!(list.total_files(), 20);
    }

    #[test]
    fn test_total_size() {
        let mut list = FolderList::new_for_test();
        list.folders.push(test_folder("/test/folder1")); // 50MB
        list.folders.push(test_folder("/test/folder2")); // 50MB

        assert_eq!(list.total_size(), 100_000_000);
    }

    // ConversionState tests

    #[test]
    fn test_conversion_state_new() {
        let state = ConversionState::new();

        assert!(!state.is_converting());
        let (completed, failed, total) = state.progress();
        assert_eq!(completed, 0);
        assert_eq!(failed, 0);
        assert_eq!(total, 0);
    }

    #[test]
    fn test_conversion_state_reset() {
        let state = ConversionState::new();

        state.reset(24);

        assert!(state.is_converting());
        let (completed, failed, total) = state.progress();
        assert_eq!(completed, 0);
        assert_eq!(failed, 0);
        assert_eq!(total, 24);
    }

    #[test]
    fn test_conversion_state_finish() {
        let state = ConversionState::new();
        state.reset(10);
        assert!(state.is_converting());

        state.finish();

        assert!(!state.is_converting());
    }

    #[test]
    fn test_conversion_state_progress_updates() {
        let state = ConversionState::new();
        state.reset(5);

        // Simulate completing some files
        state.completed.fetch_add(1, Ordering::SeqCst);
        state.completed.fetch_add(1, Ordering::SeqCst);
        state.failed.fetch_add(1, Ordering::SeqCst);

        let (completed, failed, total) = state.progress();
        assert_eq!(completed, 2);
        assert_eq!(failed, 1);
        assert_eq!(total, 5);
    }

    #[test]
    fn test_conversion_state_clone_shares_atomics() {
        let state1 = ConversionState::new();
        state1.reset(10);

        let state2 = state1.clone();

        // Update via state1
        state1.completed.fetch_add(5, Ordering::SeqCst);

        // Should be visible via state2 (shared Arc)
        let (completed, _, _) = state2.progress();
        assert_eq!(completed, 5);
    }

    #[test]
    fn test_conversion_state_thread_safety() {
        use std::thread;

        let state = ConversionState::new();
        state.reset(100);

        let mut handles = vec![];

        // Spawn 10 threads, each incrementing completed 10 times
        for _ in 0..10 {
            let state_clone = state.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..10 {
                    state_clone.completed.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let (completed, _, _) = state.progress();
        assert_eq!(completed, 100);
    }

    #[test]
    fn test_conversion_state_cancellation() {
        let state = ConversionState::new();
        state.reset(10);

        // Initially not cancelled
        assert!(!state.is_cancelled());

        // Request cancel
        state.request_cancel();

        // Should now be cancelled
        assert!(state.is_cancelled());
        // But should still be converting (in-flight tasks finish)
        assert!(state.is_converting());
    }

    #[test]
    fn test_conversion_state_reset_clears_cancel() {
        let state = ConversionState::new();
        state.reset(10);
        state.request_cancel();
        assert!(state.is_cancelled());

        // Reset should clear the cancel flag
        state.reset(5);
        assert!(!state.is_cancelled());
        assert!(state.is_converting());
    }
}
