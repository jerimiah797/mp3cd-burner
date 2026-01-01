//! FolderList component - The main application view with folder list
//!
//! This is currently the root view of the application, containing:
//! - Header
//! - Folder list with drag-and-drop
//! - Status bar

use gpui::{div, prelude::*, rgb, Context, ExternalPaths, IntoElement, Render, ScrollHandle, SharedString, Window};
use std::path::PathBuf;

use super::folder_item::{render_folder_item, DraggedFolder, FolderItemProps};
use crate::core::{format_duration, format_size, scan_music_folder, MusicFolder};
use crate::ui::Theme;

/// The main folder list view
///
/// Handles:
/// - Displaying the list of folders
/// - External drag-drop from Finder (ExternalPaths)
/// - Internal drag-drop for reordering
/// - Empty state rendering
pub struct FolderList {
    /// The list of scanned music folders
    folders: Vec<MusicFolder>,
    /// Currently hovered drop target index (for visual feedback)
    drop_target_index: Option<usize>,
    /// Whether we've subscribed to appearance changes
    appearance_subscription_set: bool,
    /// Handle for scroll state
    scroll_handle: ScrollHandle,
}

impl FolderList {
    pub fn new() -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
            appearance_subscription_set: false,
            scroll_handle: ScrollHandle::new(),
        }
    }

    /// Returns the number of folders in the list
    pub fn len(&self) -> usize {
        self.folders.len()
    }

    /// Returns true if the list is empty
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }

    /// Returns an iterator over the folders
    pub fn iter(&self) -> impl Iterator<Item = &MusicFolder> {
        self.folders.iter()
    }

    /// Check if folder path is already in the list
    fn contains_path(&self, path: &PathBuf) -> bool {
        self.folders.iter().any(|f| f.path == *path)
    }

    /// Add folders from external drop (Finder)
    ///
    /// Scans each folder and only adds directories that aren't already in the list.
    pub fn add_external_folders(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if path.is_dir() && !self.contains_path(path) {
                // Scan the folder to get metadata
                match scan_music_folder(path) {
                    Ok(folder) => {
                        println!(
                            "Scanned folder: {} ({} files, {} bytes)",
                            folder.path.display(),
                            folder.file_count,
                            folder.total_size
                        );
                        self.folders.push(folder);
                    }
                    Err(e) => {
                        eprintln!("Failed to scan folder {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    /// Add a single folder to the list
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
    pub fn clear(&mut self) {
        self.folders.clear();
    }

    /// Get all folder paths (for saving profiles, etc.)
    pub fn get_folder_paths(&self) -> Vec<PathBuf> {
        self.folders.iter().map(|f| f.path.clone()).collect()
    }

    /// Get all folders
    pub fn get_folders(&self) -> &[MusicFolder] {
        &self.folders
    }

    /// Set folders from a saved profile (re-scans each folder)
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
        let on_external_drop = cx.listener(|this, paths: &ExternalPaths, _window, _cx| {
            this.add_external_folders(paths.paths());
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
            // Right side: Convert & Burn button
            .child(
                div()
                    .id(SharedString::from("convert-burn-btn"))
                    .px_6()
                    .py_3()
                    .bg(if has_folders { success_color } else { text_muted })
                    .text_color(gpui::white())
                    .rounded_md()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_center()
                    .when(has_folders, |el| {
                        el.cursor_pointer().hover(|s| s.bg(success_hover))
                    })
                    .on_click(cx.listener(move |_this, _event, _window, _cx| {
                        if has_folders {
                            println!("Convert & Burn clicked!");
                            // TODO: Implement conversion
                        }
                    }))
                    .child("Convert\n& Burn"),
            )
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
}
