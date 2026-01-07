//! FolderItem component - A single draggable folder entry in the list

use gpui::{
    Context, Half, IntoElement, Pixels, Point, Render, SharedString, Window, div, img, prelude::*,
    px, rgb,
};
use std::path::{Path, PathBuf};

use crate::core::{FolderConversionStatus, FolderKind, MusicFolder, format_size};
use crate::ui::Theme;

/// Data carried during a drag operation for internal reordering
#[derive(Clone)]
pub struct DraggedFolder {
    /// Index of the folder being dragged
    pub index: usize,
    /// Path to the folder
    pub path: PathBuf,
    /// Album art path (if available)
    pub album_art: Option<String>,
    /// Number of files in the folder
    pub file_count: u32,
    /// Total size of files
    pub total_size: u64,
    /// Current drag position (for rendering the drag preview)
    position: Point<Pixels>,
}

impl DraggedFolder {
    pub fn new(
        index: usize,
        path: PathBuf,
        album_art: Option<String>,
        file_count: u32,
        total_size: u64,
    ) -> Self {
        Self {
            index,
            path,
            album_art,
            file_count,
            total_size,
            position: Point::default(),
        }
    }

    pub fn with_position(mut self, pos: Point<Pixels>) -> Self {
        self.position = pos;
        self
    }
}

impl Render for DraggedFolder {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::from_appearance(window.appearance());
        // Match the list item sizing dynamically based on window width
        // List items are w_full() minus px_4() padding (16px each side = 32px total)
        let viewport = window.viewport_size();
        let width = viewport.width - px(32.);
        let height = px(64.);

        let folder_name = self
            .path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.to_string_lossy().to_string());

        let file_info = format!(
            "{} files, {}",
            self.file_count,
            format_size(self.total_size)
        );

        let album_art_path = self.album_art.clone();

        div()
            .pl(self.position.x - width.half())
            .pt(self.position.y - height.half())
            .child(
                div()
                    .w(width)
                    .h(height)
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .bg(theme.bg_card)
                    .border_1()
                    .border_color(theme.accent)
                    .rounded_md()
                    .shadow_lg()
                    .opacity(0.95)
                    // Album art or folder icon
                    .child(
                        div()
                            .size_12()
                            .rounded_sm()
                            .overflow_hidden()
                            .bg(rgb(0x404040))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when_some(album_art_path, |el, path| {
                                el.child(
                                    img(Path::new(&path))
                                        .size_full()
                                        .object_fit(gpui::ObjectFit::Cover),
                                )
                            })
                            .when(self.album_art.is_none(), |el| {
                                el.child(div().text_xl().child("📁"))
                            }),
                    )
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
                                    .text_color(theme.text)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(folder_name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(file_info),
                            ),
                    ),
            )
    }
}

/// Properties for rendering a FolderItem
pub struct FolderItemProps {
    pub index: usize,
    pub folder: MusicFolder,
    pub is_drop_target: bool,
    pub theme: Theme,
    // Display settings
    pub show_file_count: bool,
    pub show_original_size: bool,
    pub show_converted_size: bool,
    pub show_source_format: bool,
    pub show_source_bitrate: bool,
    pub show_final_bitrate: bool,
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
    on_edit: impl Fn(&mut V, usize) + 'static + Clone,
) -> impl IntoElement {
    let FolderItemProps {
        index,
        folder,
        is_drop_target,
        theme,
        show_file_count,
        show_original_size,
        show_converted_size,
        show_source_format,
        show_source_bitrate,
        show_final_bitrate,
    } = props;

    // Build display name from album metadata, mixtape name, or folder name
    let folder_name = {
        // For mixtapes, use the mixtape name
        if let FolderKind::Mixtape { ref name } = folder.kind {
            name.clone()
        } else {
            let raw_folder_name = folder
                .path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| folder.path.to_string_lossy().to_string());

            // Try to build a nice display name from metadata
            match (&folder.album_name, &folder.year, &folder.artist_name) {
                // Album (Year) - Artist
                (Some(album), Some(year), Some(artist)) => {
                    format!("{} ({}) - {}", album, year, artist)
                }
                // Album (Year)
                (Some(album), Some(year), None) => {
                    format!("{} ({})", album, year)
                }
                // Album - Artist
                (Some(album), None, Some(artist)) => {
                    format!("{} - {}", album, artist)
                }
                // Just Album
                (Some(album), None, None) => album.clone(),
                // Fall back to folder name
                _ => raw_folder_name,
            }
        }
    };

    // Check if this is a mixtape (for icon and track count display)
    let is_mixtape = matches!(folder.kind, FolderKind::Mixtape { .. });

    // Calculate active track count (respects exclusions)
    let active_track_count = folder.active_tracks().len();
    let total_track_count = folder.audio_files.len();
    let has_exclusions = active_track_count < total_track_count;

    // Format metadata for display based on display settings
    // Build up parts conditionally, then join them
    let file_info = {
        let mut parts = Vec::new();

        // Source format (e.g., "FLAC" or "MP3/AAC")
        if show_source_format {
            let format = folder.source_format_summary();
            if !format.is_empty() {
                parts.push(format);
            }
        }

        // Source bitrate (e.g., "320k" or "128-320k")
        if show_source_bitrate {
            let bitrate = folder.source_bitrate_summary();
            if !bitrate.is_empty() {
                parts.push(bitrate);
            }
        }

        match &folder.conversion_status {
            FolderConversionStatus::Converted {
                output_size,
                lossless_bitrate,
                ..
            } => {
                if show_file_count {
                    // Show track count with exclusion info
                    if is_mixtape {
                        parts.push(format!("{} tracks", active_track_count));
                    } else if has_exclusions {
                        parts.push(format!("{} of {} tracks", active_track_count, total_track_count));
                    } else {
                        parts.push(format!("{} files", folder.file_count));
                    }
                }
                if show_original_size {
                    parts.push(format_size(folder.total_size));
                }
                if show_converted_size {
                    parts.push(format!("→ {}", format_size(*output_size)));
                }
                // Final bitrate after conversion (e.g., "@192k")
                if show_final_bitrate
                    && let Some(bitrate) = lossless_bitrate {
                        parts.push(format!("@{}k", bitrate));
                    }
            }
            FolderConversionStatus::Converting {
                files_completed,
                files_total,
            } => {
                // Converting state always shows progress
                parts.push(format!("{}/{} files", files_completed, files_total));
                if show_original_size {
                    parts.push(format_size(folder.total_size));
                }
                parts.push("(encoding...)".to_string());
            }
            _ => {
                if show_file_count {
                    // Show track count with exclusion info
                    if is_mixtape {
                        parts.push(format!("{} tracks", active_track_count));
                    } else if has_exclusions {
                        parts.push(format!("{} of {} tracks", active_track_count, total_track_count));
                    } else {
                        parts.push(format!("{} files", folder.file_count));
                    }
                }
                if show_original_size {
                    parts.push(format_size(folder.total_size));
                }
            }
        }

        // Add warning for folders without source
        if !folder.source_available {
            parts.push("⚠️ Source unavailable".to_string());
        }

        if parts.is_empty() {
            // If nothing to show, use empty string (folder name is always visible)
            String::new()
        } else {
            parts.join(", ")
        }
    };

    // Check if source is unavailable (affects styling)
    let source_unavailable = !folder.source_available;

    let drag_info = DraggedFolder::new(
        index,
        folder.path.clone(),
        folder.album_art.clone(),
        folder.file_count,
        folder.total_size,
    );
    let album_art_path = folder.album_art.clone();

    let on_drop_clone = on_drop.clone();

    // Theme colors
    let bg_card = theme.bg_card;
    let bg_hover = theme.bg_card_hover;
    let text_color = theme.text;
    let text_muted = theme.text_muted;
    let border_color = theme.border;
    let accent = theme.accent;
    let danger = theme.danger;
    let bg_queued = theme.bg_queued;
    let bg_queued_hover = theme.bg_queued_hover;
    let progress_line = theme.progress_line;
    let bg_warning = theme.bg_warning;
    let bg_warning_hover = theme.bg_warning_hover;
    let warning = theme.warning;

    // Determine if folder is queued for transcoding (not yet converted)
    let needs_transcoding = matches!(
        folder.conversion_status,
        FolderConversionStatus::NotConverted | FolderConversionStatus::Converting { .. }
    );

    // Calculate progress percentage for the progress line
    let progress_percent = match &folder.conversion_status {
        FolderConversionStatus::Converting {
            files_completed,
            files_total,
        } => {
            if *files_total > 0 {
                Some((*files_completed as f32 / *files_total as f32) * 100.0)
            } else {
                None
            }
        }
        _ => None,
    };

    // Outer wrapper: flex column for content + progress bar
    div()
        .id(SharedString::from(format!("folder-{}", index)))
        .w_full()
        .h_16() // Taller to fit album art
        .flex_shrink_0() // Prevent shrinking when in scrollable container
        .flex()
        .flex_col()
        .bg(if is_drop_target {
            accent
        } else if source_unavailable {
            bg_warning
        } else if needs_transcoding {
            bg_queued
        } else {
            bg_card
        })
        .border_1()
        .border_color(if is_drop_target {
            accent
        } else if source_unavailable {
            warning
        } else {
            border_color
        })
        .rounded_md()
        .overflow_hidden() // Clip progress line to rounded corners
        .cursor_grab()
        .hover(|s| {
            s.bg(if source_unavailable {
                bg_warning_hover
            } else if needs_transcoding {
                bg_queued_hover
            } else {
                bg_hover
            })
        })
        // Make this item draggable
        .on_drag(drag_info, |info: &DraggedFolder, position, _, cx| {
            cx.new(|_| info.clone().with_position(position))
        })
        // Handle internal drops (reordering)
        .on_drop(
            cx.listener(move |view, dragged: &DraggedFolder, _window, _cx| {
                on_drop_clone(view, dragged.index, index);
            }),
        )
        // Style when dragging over this item
        .drag_over::<DraggedFolder>(|style, _, _, _| style.bg(rgb(0x3d3d3d)))
        // Content row
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                // Album art or folder/mixtape icon
                .child(
                    div()
                        .size_12()
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(rgb(0x404040))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when_some(album_art_path, |el, path| {
                            el.child(
                                img(Path::new(&path))
                                    .size_full()
                                    .object_fit(gpui::ObjectFit::Cover),
                            )
                        })
                        .when(folder.album_art.is_none() && is_mixtape, |el| {
                            el.child(div().text_xl().child("🎵"))
                        })
                        .when(folder.album_art.is_none() && !is_mixtape, |el| {
                            el.child(div().text_xl().child("📁"))
                        }),
                )
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
                                .text_color(text_color)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(folder_name),
                        )
                        .child(div().text_xs().text_color(text_muted).child(file_info)),
                )
                // Edit button (pencil icon)
                .child(
                    div()
                        .id(SharedString::from(format!("edit-{}", index)))
                        .px_2()
                        .py_1()
                        .text_color(text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(accent))
                        .on_click(cx.listener(move |view, _event, _window, _cx| {
                            on_edit(view, index);
                        }))
                        .child("✎"),
                )
                // Remove button
                .child(
                    div()
                        .id(SharedString::from(format!("remove-{}", index)))
                        .px_2()
                        .py_1()
                        .text_color(text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(danger))
                        .on_click(cx.listener(move |view, _event, _window, _cx| {
                            on_remove(view, index);
                        }))
                        .child("✕"),
                ),
        )
        // Progress line at bottom (only during active conversion)
        .when_some(progress_percent, |el, pct| {
            el.child(
                div()
                    .w(gpui::relative(pct / 100.0))
                    .h(px(3.))
                    .flex_shrink_0()
                    .bg(progress_line),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dragged_folder_creation() {
        let path = PathBuf::from("/Users/test/Music/Album");
        let dragged = DraggedFolder::new(0, path.clone(), None, 10, 50_000_000);

        assert_eq!(dragged.index, 0);
        assert_eq!(dragged.path, path);
        assert_eq!(dragged.file_count, 10);
        assert_eq!(dragged.total_size, 50_000_000);
    }

    #[test]
    fn test_dragged_folder_with_position() {
        let path = PathBuf::from("/Users/test/Music/Album");
        let dragged = DraggedFolder::new(0, path, Some("/art.jpg".to_string()), 5, 25_000_000)
            .with_position(Point {
                x: px(100.),
                y: px(200.),
            });

        assert_eq!(dragged.position.x, px(100.));
        assert_eq!(dragged.position.y, px(200.));
    }
}
