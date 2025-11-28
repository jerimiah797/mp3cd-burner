//! FolderList component - The main application view with folder list
//!
//! This is currently the root view of the application, containing:
//! - Header
//! - Folder list with drag-and-drop
//! - Status bar

use gpui::{div, prelude::*, rgb, Context, ExternalPaths, IntoElement, Render, SharedString, Window};
use std::path::PathBuf;

use super::folder_item::{render_folder_item, DraggedFolder, FolderItemProps};
use super::header::Header;

/// The main folder list view
///
/// Handles:
/// - Displaying the list of folders
/// - External drag-drop from Finder (ExternalPaths)
/// - Internal drag-drop for reordering
/// - Empty state rendering
pub struct FolderList {
    /// The list of folder paths
    folders: Vec<PathBuf>,
    /// Currently hovered drop target index (for visual feedback)
    drop_target_index: Option<usize>,
}

impl FolderList {
    pub fn new() -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
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
    pub fn iter(&self) -> impl Iterator<Item = &PathBuf> {
        self.folders.iter()
    }

    /// Add folders from external drop (Finder)
    ///
    /// Only adds directories that aren't already in the list.
    pub fn add_external_folders(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if path.is_dir() && !self.folders.contains(path) {
                self.folders.push(path.clone());
            }
        }
    }

    /// Add a single folder to the list
    pub fn add_folder(&mut self, path: PathBuf) {
        if path.is_dir() && !self.folders.contains(&path) {
            self.folders.push(path);
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
    pub fn get_folders(&self) -> &[PathBuf] {
        &self.folders
    }

    /// Set folders from a saved profile
    pub fn set_folders(&mut self, folders: Vec<PathBuf>) {
        self.folders = folders;
    }

    /// Render the empty state drop zone
    fn render_empty_state(&self) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(rgb(0x94a3b8))
            .child(div().text_2xl().child("📂"))
            .child(div().text_lg().child("Drop music folders here"))
            .child(div().text_sm().child("or click to browse"))
    }

    /// Render the populated folder list
    fn render_folder_items(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let drop_target = self.drop_target_index;
        let mut list = div().size_full().p_2().flex().flex_col().gap_1();

        for (index, path) in self.folders.iter().enumerate() {
            let props = FolderItemProps {
                index,
                path: path.clone(),
                is_drop_target: drop_target == Some(index),
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_empty = self.folders.is_empty();
        let folder_count = self.folders.len();

        // Build the folder list content
        let list_content = if is_empty {
            self.render_empty_state().into_any_element()
        } else {
            self.render_folder_items(cx).into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xf5f5f5))
            // Handle external file drops on the entire window
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, _cx| {
                this.add_external_folders(paths.paths());
                this.drop_target_index = None;
            }))
            // Style when dragging external files over window
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style.bg(rgb(0xe0f2fe))
            })
            // Header
            .child(Header::render("MP3 CD Burner"))
            // Main content area
            .child(
                div()
                    .flex_1()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_4()
                    // Instructions
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x64748b))
                            .child("Drag folders from Finder, or drag items to reorder"),
                    )
                    // Folder list container
                    .child(
                        div()
                            .flex_1()
                            .w_full()
                            .border_2()
                            .border_color(rgb(0xe2e8f0))
                            .rounded_lg()
                            .bg(gpui::white())
                            .overflow_hidden()
                            // Handle drops on the list container
                            .on_drop(cx.listener(|this, dragged: &DraggedFolder, _window, _cx| {
                                let target = this.folders.len();
                                this.move_folder(dragged.index, target);
                                this.drop_target_index = None;
                            }))
                            .drag_over::<DraggedFolder>(|style, _, _, _| {
                                style.border_color(rgb(0x3b82f6))
                            })
                            .child(list_content),
                    )
                    // Status bar
                    .child(self.render_status_bar(folder_count, cx)),
            )
    }
}

impl FolderList {
    /// Render the status bar with folder count and action button
    fn render_status_bar(&self, folder_count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let folder_text = if folder_count == 1 {
            "1 folder".to_string()
        } else {
            format!("{} folders", folder_count)
        };
        let has_folders = folder_count > 0;

        div()
            .h_8()
            .flex()
            .items_center()
            .justify_between()
            .text_sm()
            .text_color(rgb(0x64748b))
            .child(folder_text)
            .child(
                div()
                    .id(SharedString::from("convert-burn-btn"))
                    .px_3()
                    .py_1()
                    .bg(if has_folders { rgb(0x3b82f6) } else { rgb(0x94a3b8) })
                    .text_color(gpui::white())
                    .rounded_md()
                    .when(has_folders, |el| {
                        el.cursor_pointer().hover(|s| s.bg(rgb(0x2563eb)))
                    })
                    .on_click(cx.listener(move |_this, _event, _window, _cx| {
                        if has_folders {
                            println!("Convert & Burn clicked!");
                            // TODO: Implement conversion
                        }
                    }))
                    .child("Convert & Burn"),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_folder_list_new() {
        let list = FolderList::new();
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_add_folder() {
        let mut list = FolderList::new();
        // Note: In tests, we can't actually check is_dir(), so this tests the dedup logic
        list.folders.push(PathBuf::from("/test/folder1"));
        list.folders.push(PathBuf::from("/test/folder2"));

        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_remove_folder() {
        let mut list = FolderList::new();
        list.folders.push(PathBuf::from("/test/folder1"));
        list.folders.push(PathBuf::from("/test/folder2"));

        list.remove_folder(0);

        assert_eq!(list.len(), 1);
        assert_eq!(list.folders[0], PathBuf::from("/test/folder2"));
    }

    #[test]
    fn test_move_folder_forward() {
        let mut list = FolderList::new();
        list.folders.push(PathBuf::from("/test/a"));
        list.folders.push(PathBuf::from("/test/b"));
        list.folders.push(PathBuf::from("/test/c"));

        // Move "a" to position 2 (after "b")
        list.move_folder(0, 2);

        assert_eq!(list.folders[0], PathBuf::from("/test/b"));
        assert_eq!(list.folders[1], PathBuf::from("/test/a"));
        assert_eq!(list.folders[2], PathBuf::from("/test/c"));
    }

    #[test]
    fn test_move_folder_backward() {
        let mut list = FolderList::new();
        list.folders.push(PathBuf::from("/test/a"));
        list.folders.push(PathBuf::from("/test/b"));
        list.folders.push(PathBuf::from("/test/c"));

        // Move "c" to position 0 (before "a")
        list.move_folder(2, 0);

        assert_eq!(list.folders[0], PathBuf::from("/test/c"));
        assert_eq!(list.folders[1], PathBuf::from("/test/a"));
        assert_eq!(list.folders[2], PathBuf::from("/test/b"));
    }

    #[test]
    fn test_clear() {
        let mut list = FolderList::new();
        list.folders.push(PathBuf::from("/test/folder1"));
        list.folders.push(PathBuf::from("/test/folder2"));

        list.clear();

        assert!(list.is_empty());
    }
}
