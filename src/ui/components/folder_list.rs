//! FolderList component - The main application view with folder list
//!
//! This is currently the root view of the application, containing:
//! - Header
//! - Folder list with drag-and-drop
//! - Status bar

use gpui::{div, prelude::*, rgb, AsyncApp, Context, ExternalPaths, IntoElement, Render, ScrollHandle, SharedString, Timer, WeakEntity, Window};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::folder_item::{render_folder_item, DraggedFolder, FolderItemProps};
use crate::conversion::{
    ensure_output_dir, verify_ffmpeg, convert_files_parallel_with_callback, ConversionJob, ConversionProgress,
};
use crate::core::{format_duration, get_audio_files, scan_music_folder, MusicFolder};
use crate::ui::Theme;

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
    /// Number of files completed
    pub completed: Arc<AtomicUsize>,
    /// Number of files failed
    pub failed: Arc<AtomicUsize>,
    /// Total number of files to convert
    pub total: Arc<AtomicUsize>,
}

impl ConversionState {
    pub fn new() -> Self {
        Self {
            is_converting: Arc::new(AtomicBool::new(false)),
            completed: Arc::new(AtomicUsize::new(0)),
            failed: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn reset(&self, total: usize) {
        self.is_converting.store(true, Ordering::SeqCst);
        self.completed.store(0, Ordering::SeqCst);
        self.failed.store(0, Ordering::SeqCst);
        self.total.store(total, Ordering::SeqCst);
    }

    pub fn finish(&self) {
        self.is_converting.store(false, Ordering::SeqCst);
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
}

impl FolderList {
    pub fn new() -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
            appearance_subscription_set: false,
            scroll_handle: ScrollHandle::new(),
            conversion_state: ConversionState::new(),
            import_state: ImportState::new(),
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

        // Reset import state
        self.import_state.reset(new_paths.len());

        // Clone state for background thread
        let state = self.import_state.clone();

        // Spawn background thread for scanning
        std::thread::spawn(move || {
            for path in new_paths {
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
    /// Returns bitrate in kbps
    pub fn calculated_bitrate(&self) -> u32 {
        let duration = self.total_duration();
        if duration <= 0.0 {
            return 320; // Default to max if no duration
        }

        // Target size: 700MB with 80% overhead compensation
        let target_bytes = 700.0 * 1024.0 * 1024.0 * 0.80;
        // bitrate = (bytes * 8) / (seconds * 1000)
        let bitrate = (target_bytes * 8.0) / (duration * 1000.0);

        // Clamp between 64 and 320 kbps
        (bitrate as u32).clamp(64, 320)
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
        let mut list = div().w_full().flex().flex_col().gap_1();

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

impl Default for FolderList {
    fn default() -> Self {
        Self::new()
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

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg)
            // Handle external file drops on the entire window
            .on_drop(on_external_drop)
            // Style when dragging external files over window
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style.bg(rgb(0x3d3d3d))
            })
            // Main content area - folder list (scrollable)
            .child(
                div()
                    .id("folder-list-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll_handle)
                    .px_4() // Horizontal padding for breathing room
                    .py_2() // Vertical padding
                    // Handle drops on the list container
                    .on_drop(on_internal_drop)
                    .drag_over::<DraggedFolder>(|style, _, _, _| {
                        style.bg(rgb(0x3d3d3d))
                    })
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
        let bitrate = self.calculated_bitrate();
        let has_folders = !self.folders.is_empty();

        let success_color = theme.success;
        let success_hover = theme.success_hover;
        let text_muted = theme.text_muted;
        let text_color = theme.text;
        let bg = theme.bg;

        // Format size in MB
        let size_mb = total_size as f64 / (1024.0 * 1024.0);

        div()
            .py_2()
            .px_4()
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
                    // Row 3: Bitrate (in accent/success color)
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child("Bitrate:")
                            .child(
                                div()
                                    .text_color(success_color)
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(format!("{} kbps", bitrate)),
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
                    // Show progress bar during conversion
                    let progress_fraction = if total > 0 {
                        (completed + failed) as f32 / total as f32
                    } else {
                        0.0
                    };

                    div()
                        .id(SharedString::from("convert-progress"))
                        .w(gpui::px(140.0))
                        .h(gpui::px(70.0))
                        .rounded_md()
                        .border_1()
                        .border_color(success_color)
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
                                .bg(success_color)
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
                                        .child(format!("{}/{}", completed + failed, total))
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(gpui::white())
                                        .child("Converting...")
                                )
                        )
                } else {
                    // Normal Convert & Burn button
                    div()
                        .id(SharedString::from("convert-burn-btn"))
                        .px_8()
                        .py_4()
                        .bg(if has_folders { success_color } else { text_muted })
                        .text_color(gpui::white())
                        .text_lg()
                        .rounded_md()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_center()
                        .when(has_folders, |el| {
                            el.cursor_pointer().hover(|s| s.bg(success_hover))
                        })
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            if has_folders {
                                println!("Convert & Burn clicked!");
                                this.run_conversion(cx);
                            }
                        }))
                        .child("Convert\n& Burn")
                }
            })
    }

    /// Run the conversion process for all folders (async, in background thread)
    fn run_conversion(&mut self, cx: &mut Context<Self>) {
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

        // Calculate target bitrate
        let bitrate = self.calculated_bitrate();
        println!("Target bitrate: {} kbps", bitrate);

        // Build list of conversion jobs
        let mut jobs: Vec<ConversionJob> = Vec::new();

        for (folder_idx, folder) in self.folders.iter().enumerate() {
            // Create numbered album folder (01-AlbumName, 02-AlbumName, etc.)
            let folder_name = folder.path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown");
            let album_dir_name = format!("{:02}-{}", folder_idx + 1, folder_name);
            let album_output_dir = output_dir.join(&album_dir_name);

            println!("Preparing folder: {} -> {}", folder.path.display(), album_dir_name);

            // Get all audio files in this folder
            let audio_files = match get_audio_files(&folder.path) {
                Ok(files) => files,
                Err(e) => {
                    eprintln!("Failed to get audio files from {}: {}", folder.path.display(), e);
                    continue;
                }
            };

            // Create a job for each file
            for audio_file in audio_files {
                let file_stem = audio_file.path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output");
                let output_path = album_output_dir.join(format!("{}.mp3", file_stem));

                jobs.push(ConversionJob {
                    input_path: audio_file.path,
                    output_path,
                });
            }
        }

        let total_jobs = jobs.len();
        if total_jobs == 0 {
            println!("No files to convert");
            return;
        }

        // Reset conversion state
        self.conversion_state.reset(total_jobs);

        // Clone state for the background thread
        let state = self.conversion_state.clone();

        // Spawn background thread with tokio runtime
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

            rt.block_on(async {
                let progress = Arc::new(ConversionProgress::new(total_jobs));
                let progress_for_callback = progress.clone();
                let state_for_callback = state.clone();

                // Use callback version to sync progress to ConversionState after each file
                let (completed, failed) = convert_files_parallel_with_callback(
                    ffmpeg_path,
                    jobs,
                    bitrate,
                    progress,
                    move || {
                        // Sync atomics from ConversionProgress to ConversionState
                        let completed = progress_for_callback.completed_count();
                        let failed = progress_for_callback.failed_count();
                        state_for_callback.completed.store(completed, Ordering::SeqCst);
                        state_for_callback.failed.store(failed, Ordering::SeqCst);
                    },
                ).await;

                println!("Conversion complete: {} converted, {} failed", completed, failed);
                state.finish();
            });
        });

        // Start polling for progress updates - pass state directly to avoid reading entity
        Self::start_progress_polling(self.conversion_state.clone(), cx);

        println!("Conversion started in background ({} files)", total_jobs);
        cx.notify(); // Initial notification to show 0/N
    }

    /// Start a polling loop that updates the UI periodically during conversion
    fn start_progress_polling(state: ConversionState, cx: &mut Context<Self>) {
        // state is already cloned - no need to read entity

        // Clone in sync part BEFORE the async block - key to avoiding lifetime issues
        cx.spawn(|_this: WeakEntity<Self>, cx: &mut AsyncApp| {
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
        }
    }

    #[test]
    fn test_folder_list_new() {
        let list = FolderList::new();
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_add_folder() {
        let mut list = FolderList::new();
        list.folders.push(test_folder("/test/folder1"));
        list.folders.push(test_folder("/test/folder2"));

        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_remove_folder() {
        let mut list = FolderList::new();
        list.folders.push(test_folder("/test/folder1"));
        list.folders.push(test_folder("/test/folder2"));

        list.remove_folder(0);

        assert_eq!(list.len(), 1);
        assert_eq!(list.folders[0].path, PathBuf::from("/test/folder2"));
    }

    #[test]
    fn test_move_folder_forward() {
        let mut list = FolderList::new();
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
        let mut list = FolderList::new();
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
        let mut list = FolderList::new();
        list.folders.push(test_folder("/test/folder1"));
        list.folders.push(test_folder("/test/folder2"));

        list.clear();

        assert!(list.is_empty());
    }

    #[test]
    fn test_total_files() {
        let mut list = FolderList::new();
        list.folders.push(test_folder("/test/folder1")); // 10 files
        list.folders.push(test_folder("/test/folder2")); // 10 files

        assert_eq!(list.total_files(), 20);
    }

    #[test]
    fn test_total_size() {
        let mut list = FolderList::new();
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
}
