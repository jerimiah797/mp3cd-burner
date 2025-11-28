//! FolderItem component - A single draggable folder entry in the list

use gpui::{
    div, prelude::*, px, rgb, rgba, Context, Half, IntoElement, Pixels, Point, Render,
    SharedString, Window,
};
use std::path::PathBuf;

use crate::core::{format_size, MusicFolder};

/// Data carried during a drag operation for internal reordering
#[derive(Clone)]
pub struct DraggedFolder {
    /// Index of the folder being dragged
    pub index: usize,
    /// Path to the folder
    pub path: PathBuf,
    /// Current drag position (for rendering the drag preview)
    position: Point<Pixels>,
}

impl DraggedFolder {
    pub fn new(index: usize, path: PathBuf) -> Self {
        Self {
            index,
            path,
            position: Point::default(),
        }
    }

    pub fn with_position(mut self, pos: Point<Pixels>) -> Self {
        self.position = pos;
        self
    }
}

impl Render for DraggedFolder {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let size = gpui::size(px(250.), px(40.));
        let folder_name = self
            .path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.to_string_lossy().to_string());

        div()
            .pl(self.position.x - size.width.half())
            .pt(self.position.y - size.height.half())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w(size.width)
                    .h(size.height)
                    .px_3()
                    .bg(rgba(0x2563ebee))
                    .text_color(gpui::white())
                    .text_sm()
                    .rounded_md()
                    .shadow_lg()
                    .child("📁")
                    .child(folder_name),
            )
    }
}

/// Properties for rendering a FolderItem
pub struct FolderItemProps {
    pub index: usize,
    pub folder: MusicFolder,
    pub is_drop_target: bool,
}

/// Renders a single folder item in the list
///
/// This is a stateless render function rather than a component because
/// the item's state (path, index) is owned by the parent FolderList.
pub fn render_folder_item<V: 'static>(
    props: FolderItemProps,
    cx: &mut Context<V>,
    on_drop: impl Fn(&mut V, usize, usize) + 'static + Clone,
    on_remove: impl Fn(&mut V, usize) + 'static + Clone,
) -> impl IntoElement {
    let FolderItemProps {
        index,
        folder,
        is_drop_target,
    } = props;

    let folder_name = folder
        .path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| folder.path.to_string_lossy().to_string());

    // Format metadata for display
    let file_info = format!(
        "{} files, {}",
        folder.file_count,
        format_size(folder.total_size)
    );

    let drag_info = DraggedFolder::new(index, folder.path.clone());

    let on_drop_clone = on_drop.clone();

    div()
        .id(SharedString::from(format!("folder-{}", index)))
        .w_full()
        .h_12() // Slightly taller to accommodate two lines
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .bg(if is_drop_target {
            rgb(0xdbeafe)
        } else {
            rgb(0xffffff)
        })
        .border_1()
        .border_color(if is_drop_target {
            rgb(0x3b82f6)
        } else {
            rgb(0xe2e8f0)
        })
        .rounded_md()
        .cursor_grab()
        .hover(|s| s.bg(rgb(0xf8fafc)))
        // Make this item draggable
        .on_drag(drag_info, |info: &DraggedFolder, position, _, cx| {
            cx.new(|_| info.clone().with_position(position))
        })
        // Handle internal drops (reordering)
        .on_drop(cx.listener(move |view, dragged: &DraggedFolder, _window, _cx| {
            on_drop_clone(view, dragged.index, index);
        }))
        // Style when dragging over this item
        .drag_over::<DraggedFolder>(|style, _, _, _| {
            style.bg(rgb(0xdbeafe)).border_color(rgb(0x3b82f6))
        })
        // Drag handle
        .child(div().text_color(rgb(0x94a3b8)).child("⋮⋮"))
        // Folder icon
        .child(div().child("📁"))
        // Folder name and metadata
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x1e293b))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(folder_name),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x94a3b8))
                        .child(file_info),
                ),
        )
        // Remove button
        .child(
            div()
                .id(SharedString::from(format!("remove-{}", index)))
                .px_2()
                .text_color(rgb(0x94a3b8))
                .cursor_pointer()
                .hover(|s| s.text_color(rgb(0xef4444)))
                .on_click(cx.listener(move |view, _event, _window, _cx| {
                    on_remove(view, index);
                }))
                .child("✕"),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dragged_folder_creation() {
        let path = PathBuf::from("/Users/test/Music/Album");
        let dragged = DraggedFolder::new(0, path.clone());

        assert_eq!(dragged.index, 0);
        assert_eq!(dragged.path, path);
    }

    #[test]
    fn test_dragged_folder_with_position() {
        let path = PathBuf::from("/Users/test/Music/Album");
        let dragged = DraggedFolder::new(0, path).with_position(Point {
            x: px(100.),
            y: px(200.),
        });

        assert_eq!(dragged.position.x, px(100.));
        assert_eq!(dragged.position.y, px(200.));
    }
}
