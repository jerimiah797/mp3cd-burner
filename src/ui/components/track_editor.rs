//! Track Editor Window
//!
//! A unified editor window for managing tracks in both album and mixtape folders.
//! - Album mode: View tracks, toggle include/exclude, reorder to fix metadata issues
//! - Mixtape mode: Add tracks from Finder, reorder, remove, rename mixtape

use gpui::{
    Bounds, Context, ExternalPaths, FocusHandle, Half, IntoElement, KeyDownEvent, Pixels, Point,
    Render, SharedString, Window, WindowBounds, WindowOptions, div, img, prelude::*, px, rgb,
    size,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::audio::{get_album_art, get_audio_metadata, is_audio_file};
use crate::core::{AudioFileInfo, FolderId, FolderKind, format_duration};
use crate::ui::Theme;

/// A single track entry in the editor
#[derive(Debug, Clone)]
pub struct TrackEntry {
    /// The audio file info
    pub file_info: AudioFileInfo,
    /// Album art path (for mixtapes, each track may have different art)
    pub album_art: Option<String>,
    /// Whether this track is included (for albums - excluded tracks are dimmed)
    pub included: bool,
}

/// Data carried during a drag operation for track reordering
#[derive(Clone)]
pub struct DraggedTrack {
    /// Index of the track being dragged
    pub index: usize,
    /// Track display name
    pub name: String,
    /// Current drag position
    position: Point<Pixels>,
    /// Source window title (to avoid rendering in wrong windows)
    source_window_title: String,
}

impl DraggedTrack {
    pub fn new(index: usize, name: String, window_title: String) -> Self {
        Self {
            index,
            name,
            position: Point::default(),
            source_window_title: window_title,
        }
    }

    pub fn with_position(mut self, pos: Point<Pixels>) -> Self {
        self.position = pos;
        self
    }
}

impl Render for DraggedTrack {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Only render in the window that matches our source window title
        // This prevents the drag preview from appearing in other windows
        if window.window_title() != self.source_window_title {
            return div().into_any_element();
        }

        let theme = Theme::from_appearance(window.appearance());
        let viewport = window.viewport_size();
        let width = viewport.width - px(48.);
        let height = px(40.);

        div()
            .pl(self.position.x - width.half())
            .pt(self.position.y - height.half())
            .child(
                div()
                    .w(width)
                    .h(height)
                    .flex()
                    .items_center()
                    .px_3()
                    .bg(theme.bg_card)
                    .border_1()
                    .border_color(theme.accent)
                    .rounded_md()
                    .shadow_lg()
                    .opacity(0.95)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(self.name.clone()),
                    ),
            )
            .into_any_element()
    }
}

/// Updates sent from the track editor to the parent FolderList
#[derive(Debug, Clone)]
pub enum TrackEditorUpdate {
    /// Track order changed via drag reorder
    OrderChanged { id: FolderId, order: Vec<usize> },
    /// Exclusions changed (albums only)
    ExclusionsChanged { id: FolderId, excluded: Vec<PathBuf> },
    /// Tracks completely changed (mixtapes - add/remove)
    TracksChanged {
        id: FolderId,
        tracks: Vec<AudioFileInfo>,
        album_arts: Vec<Option<String>>,
    },
    /// Mixtape name changed
    NameChanged { id: FolderId, name: String },
    /// Editor window closed
    Closed { id: FolderId },
}

/// The Track Editor window
pub struct TrackEditorWindow {
    /// ID of the folder being edited
    folder_id: FolderId,
    /// Kind of folder (Album or Mixtape)
    folder_kind: FolderKind,
    /// Display name (album name or mixtape name)
    name: String,
    /// Original name (for detecting changes)
    original_name: String,
    /// Whether we're editing the name (mixtapes only)
    editing_name: bool,
    /// Tracks in the editor
    tracks: Vec<TrackEntry>,
    /// Original track order (indices) - for Reset Order and detecting changes
    original_order: Vec<usize>,
    /// Original inclusion state for each track (for detecting changes)
    original_inclusions: Vec<bool>,
    /// Current track order (indices into tracks vec)
    track_order: Vec<usize>,
    /// Index of drop target during drag
    drop_target: Option<usize>,
    /// Channel to send updates to FolderList
    update_tx: mpsc::Sender<TrackEditorUpdate>,
    /// Focus handle for keyboard input
    focus_handle: FocusHandle,
    /// Scroll position for track list
    scroll_offset: f32,
}

impl TrackEditorWindow {
    /// Create a new track editor
    pub fn new(
        cx: &mut Context<Self>,
        folder_id: FolderId,
        folder_kind: FolderKind,
        name: String,
        tracks: Vec<TrackEntry>,
        update_tx: mpsc::Sender<TrackEditorUpdate>,
        existing_track_order: Option<Vec<usize>>,
    ) -> Self {
        let track_count = tracks.len();
        // Use existing track order if provided, otherwise use default sequential order
        let track_order: Vec<usize> =
            existing_track_order.unwrap_or_else(|| (0..track_count).collect());
        let original_order = track_order.clone();
        let original_inclusions: Vec<bool> = tracks.iter().map(|t| t.included).collect();

        Self {
            folder_id,
            folder_kind,
            name: name.clone(),
            original_name: name,
            editing_name: false,
            tracks,
            original_order,
            original_inclusions,
            track_order,
            drop_target: None,
            update_tx,
            focus_handle: cx.focus_handle(),
            scroll_offset: 0.0,
        }
    }

    /// Open the track editor window
    pub fn open(
        cx: &mut gpui::App,
        folder_id: FolderId,
        folder_kind: FolderKind,
        name: String,
        tracks: Vec<TrackEntry>,
        update_tx: mpsc::Sender<TrackEditorUpdate>,
        existing_track_order: Option<Vec<usize>>,
    ) -> gpui::WindowHandle<Self> {
        let title = match &folder_kind {
            FolderKind::Album => format!("{} - Track Editor", name),
            FolderKind::Mixtape { .. } => "Mixtape Editor".to_string(),
        };

        let bounds = Bounds::centered(None, size(px(600.), px(650.)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(400.), px(300.))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(title.into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|cx| {
                    TrackEditorWindow::new(
                        cx,
                        folder_id,
                        folder_kind,
                        name,
                        tracks,
                        update_tx,
                        existing_track_order,
                    )
                })
            },
        )
        .unwrap()
    }

    /// Check if this is a mixtape
    fn is_mixtape(&self) -> bool {
        matches!(self.folder_kind, FolderKind::Mixtape { .. })
    }

    /// Get the display name for a track
    fn track_display_name(&self, track: &TrackEntry) -> String {
        track
            .file_info
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    /// Handle key press
    fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let keystroke = &event.keystroke;

        if keystroke.key == "escape" {
            if self.editing_name {
                self.editing_name = false;
                cx.notify();
                return true;
            }
            self.cancel(window, cx);
            return true;
        }

        if keystroke.key == "enter" && self.editing_name {
            self.editing_name = false;
            cx.notify();
            return true;
        }

        // Handle name editing input
        if self.editing_name {
            if keystroke.key == "backspace" && !self.name.is_empty() {
                self.name.pop();
                cx.notify();
                return true;
            }
            if let Some(ref key_char) = keystroke.key_char {
                for c in key_char.chars() {
                    if !c.is_control() {
                        self.name.push(c);
                    }
                }
                cx.notify();
                return true;
            }
        }

        false
    }

    /// Toggle track inclusion (album mode)
    fn toggle_track(&mut self, track_index: usize, cx: &mut Context<Self>) {
        if let Some(track) = self.tracks.get_mut(track_index) {
            track.included = !track.included;
            cx.notify();
        }
    }

    /// Remove a track (mixtape mode)
    fn remove_track(&mut self, track_index: usize, cx: &mut Context<Self>) {
        if track_index < self.tracks.len() {
            self.tracks.remove(track_index);
            // Rebuild track order
            self.track_order = (0..self.tracks.len()).collect();
            self.original_order = self.track_order.clone();
            self.send_tracks_update();
            cx.notify();
        }
    }

    /// Move a track from one position to another
    fn move_track(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to || from >= self.track_order.len() || to >= self.track_order.len() {
            return;
        }

        let track_idx = self.track_order.remove(from);
        self.track_order.insert(to, track_idx);
        cx.notify();
    }

    /// Reset track order to original
    fn reset_order(&mut self, cx: &mut Context<Self>) {
        self.track_order = self.original_order.clone();
        cx.notify();
    }

    /// Handle external file drop (mixtapes only)
    fn handle_external_drop(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if !self.is_mixtape() {
            return;
        }

        let mut added_any = false;

        for path in paths {
            if path.is_dir() {
                // Recursively scan directory for audio files
                self.add_files_from_directory(path);
                added_any = true;
            } else if path.is_file() && is_audio_file(path) {
                self.add_audio_file(path);
                added_any = true;
            }
        }

        if added_any {
            // Rebuild order arrays
            self.track_order = (0..self.tracks.len()).collect();
            self.original_order = self.track_order.clone();
            self.send_tracks_update();
            cx.notify();
        }
    }

    /// Add audio files from a directory recursively
    fn add_files_from_directory(&mut self, dir: &Path) {
        use walkdir::WalkDir;

        let mut files: Vec<PathBuf> = WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file() && is_audio_file(e.path()))
            .map(|e| e.path().to_path_buf())
            .collect();

        // Sort by filename for consistent ordering
        files.sort();

        for file in files {
            self.add_audio_file(&file);
        }
    }

    /// Add a single audio file to the mixtape
    fn add_audio_file(&mut self, path: &Path) {
        // Check if already added
        if self.tracks.iter().any(|t| t.file_info.path == path) {
            return;
        }

        // Get audio metadata
        if let Ok((duration, bitrate, codec, is_lossy)) = get_audio_metadata(path) {
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let album_art = get_album_art(path);

            let file_info = AudioFileInfo {
                path: path.to_path_buf(),
                duration,
                bitrate,
                size,
                codec,
                is_lossy,
            };

            self.tracks.push(TrackEntry {
                file_info,
                album_art,
                included: true,
            });
        }
    }

    /// Send tracks update to parent (mixtapes)
    fn send_tracks_update(&self) {
        // Build tracks in current order
        let tracks: Vec<AudioFileInfo> = self
            .track_order
            .iter()
            .filter_map(|&i| self.tracks.get(i))
            .map(|t| t.file_info.clone())
            .collect();

        let album_arts: Vec<Option<String>> = self
            .track_order
            .iter()
            .filter_map(|&i| self.tracks.get(i))
            .map(|t| t.album_art.clone())
            .collect();

        let _ = self.update_tx.send(TrackEditorUpdate::TracksChanged {
            id: self.folder_id.clone(),
            tracks,
            album_arts,
        });
    }

    /// Check if there are unsaved changes
    fn has_changes(&self) -> bool {
        // Check track order
        if self.track_order != self.original_order {
            return true;
        }

        // Check exclusions
        let current_inclusions: Vec<bool> = self.tracks.iter().map(|t| t.included).collect();
        if current_inclusions != self.original_inclusions {
            return true;
        }

        // Check name (mixtapes)
        if self.name != self.original_name {
            return true;
        }

        // For mixtapes, check if tracks were added/removed
        if self.is_mixtape() && self.tracks.len() != self.original_inclusions.len() {
            return true;
        }

        false
    }

    /// Apply all changes and close the editor
    fn apply_and_close(&self, window: &mut Window, _cx: &mut Context<Self>) {
        // Send order change if order changed
        if self.track_order != self.original_order {
            let _ = self.update_tx.send(TrackEditorUpdate::OrderChanged {
                id: self.folder_id.clone(),
                order: self.track_order.clone(),
            });
        }

        // Send exclusions change if inclusions changed
        let current_inclusions: Vec<bool> = self.tracks.iter().map(|t| t.included).collect();
        if current_inclusions != self.original_inclusions {
            let excluded: Vec<PathBuf> = self
                .tracks
                .iter()
                .filter(|t| !t.included)
                .map(|t| t.file_info.path.clone())
                .collect();

            let _ = self.update_tx.send(TrackEditorUpdate::ExclusionsChanged {
                id: self.folder_id.clone(),
                excluded,
            });
        }

        // Send name change if name changed (mixtapes)
        if self.name != self.original_name {
            let _ = self.update_tx.send(TrackEditorUpdate::NameChanged {
                id: self.folder_id.clone(),
                name: self.name.clone(),
            });
        }

        // For mixtapes, send tracks change if tracks were added/removed
        if self.is_mixtape() && self.tracks.len() != self.original_inclusions.len() {
            let tracks: Vec<AudioFileInfo> = self
                .track_order
                .iter()
                .filter_map(|&i| self.tracks.get(i))
                .map(|t| t.file_info.clone())
                .collect();

            let album_arts: Vec<Option<String>> = self
                .track_order
                .iter()
                .filter_map(|&i| self.tracks.get(i))
                .map(|t| t.album_art.clone())
                .collect();

            let _ = self.update_tx.send(TrackEditorUpdate::TracksChanged {
                id: self.folder_id.clone(),
                tracks,
                album_arts,
            });
        }

        // Always send closed event
        let _ = self.update_tx.send(TrackEditorUpdate::Closed {
            id: self.folder_id.clone(),
        });
        window.remove_window();
    }

    /// Cancel changes and close the editor
    fn cancel(&self, window: &mut Window, _cx: &mut Context<Self>) {
        // Just send closed event without any changes
        let _ = self.update_tx.send(TrackEditorUpdate::Closed {
            id: self.folder_id.clone(),
        });
        window.remove_window();
    }

    /// Render a single track row
    fn render_track_row(
        &self,
        display_index: usize,
        track_index: usize,
        track: &TrackEntry,
        theme: &Theme,
        window_title: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = self.track_display_name(track);
        let duration = format_duration(track.file_info.duration);
        let format_badge = track.file_info.codec.to_uppercase();
        let is_lossy = track.file_info.is_lossy;
        let included = track.included;
        let is_drop_target = self.drop_target == Some(display_index);
        let is_mixtape = self.is_mixtape();
        let album_art = track.album_art.clone();

        let drag_info = DraggedTrack::new(display_index, name.clone(), window_title.to_string());

        let bg_color = if is_drop_target {
            theme.accent
        } else if !included {
            theme.bg_card.opacity(0.5)
        } else {
            theme.bg_card
        };

        let text_color = if included {
            theme.text
        } else {
            theme.text_muted
        };

        div()
            .id(SharedString::from(format!("track-{}", display_index)))
            .w_full()
            .h_10()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(bg_color)
            .border_1()
            .border_color(if is_drop_target {
                theme.accent
            } else {
                theme.border
            })
            .rounded_md()
            .cursor_grab()
            .hover(|s| s.bg(theme.bg_card_hover))
            // Make draggable for reordering
            .on_drag(drag_info, |info: &DraggedTrack, position, _, cx| {
                cx.new(|_| info.clone().with_position(position))
            })
            // Handle drops for reordering
            .on_drop(cx.listener(move |this, dragged: &DraggedTrack, _window, cx| {
                this.drop_target = None;
                this.move_track(dragged.index, display_index, cx);
            }))
            .drag_over::<DraggedTrack>(|style, _, _, _| style.bg(rgb(0x3d3d3d)))
            // Track number
            .child(
                div()
                    .w_6()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .text_center()
                    .child(format!("{}", display_index + 1)),
            )
            // Album art thumbnail (for mixtapes)
            .when(is_mixtape, |el| {
                el.child(
                    div()
                        .size_8()
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(rgb(0x404040))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when_some(album_art, |el, path| {
                            el.child(
                                img(Path::new(&path))
                                    .size_full()
                                    .object_fit(gpui::ObjectFit::Cover),
                            )
                        }),
                )
            })
            // Include/exclude checkbox (album mode only)
            .when(!is_mixtape, |el| {
                let track_idx = track_index;
                el.child(
                    div()
                        .id(SharedString::from(format!("checkbox-{}", display_index)))
                        .size_5()
                        .rounded_sm()
                        .border_1()
                        .border_color(theme.border)
                        .bg(if included {
                            theme.accent
                        } else {
                            theme.bg_card
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.toggle_track(track_idx, cx);
                        }))
                        .when(included, |el| {
                            el.child(div().text_xs().text_color(gpui::white()).child("✓"))
                        }),
                )
            })
            // Track name
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(text_color)
                    .overflow_hidden()
                    .text_ellipsis()
                    .when(!included, |el| el.line_through())
                    .child(name),
            )
            // Format badge
            .child(
                div()
                    .px_2()
                    .py_px()
                    .text_xs()
                    .rounded_sm()
                    .bg(if is_lossy {
                        theme.warning.opacity(0.2)
                    } else {
                        theme.success.opacity(0.2)
                    })
                    .text_color(if is_lossy {
                        theme.warning
                    } else {
                        theme.success
                    })
                    .child(format_badge),
            )
            // Duration
            .child(
                div()
                    .w_12()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .text_right()
                    .child(duration),
            )
            // Remove button (mixtape mode only)
            .when(is_mixtape, |el| {
                let track_idx = track_index;
                el.child(
                    div()
                        .id(SharedString::from(format!("remove-{}", display_index)))
                        .px_2()
                        .py_1()
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.danger))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.remove_track(track_idx, cx);
                        }))
                        .child("✕"),
                )
            })
    }
}

impl Render for TrackEditorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::from_appearance(window.appearance());
        let window_title = window.window_title();
        let is_mixtape = self.is_mixtape();
        let name = self.name.clone();
        let editing_name = self.editing_name;
        let track_count = self.tracks.len();
        let included_count = self.tracks.iter().filter(|t| t.included).count();
        let has_changes = self.has_changes();

        // Calculate total duration
        let total_duration: f64 = self
            .track_order
            .iter()
            .filter_map(|&i| self.tracks.get(i))
            .filter(|t| t.included)
            .map(|t| t.file_info.duration)
            .sum();

        // Focus on render
        if !self.focus_handle.is_focused(window) {
            self.focus_handle.focus(window);
        }

        div()
            .key_context("TrackEditorWindow")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg)
            // Handle external drops (mixtapes only)
            .when(is_mixtape, |el| {
                el.on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                    this.handle_external_drop(paths.paths(), cx);
                }))
                .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(0x3d3d3d)))
            })
            // Header
            .child(
                div()
                    .w_full()
                    .p_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme.border)
                    // Name display/editor
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(if is_mixtape && editing_name {
                                // Editable name input
                                div()
                                    .flex_1()
                                    .h_8()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .bg(theme.bg_card)
                                    .border_1()
                                    .border_color(theme.accent)
                                    .rounded_md()
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.text)
                                            .child(if name.is_empty() {
                                                " ".to_string()
                                            } else {
                                                name.clone()
                                            }),
                                    )
                                    .child(div().w(px(2.)).h(px(16.)).bg(theme.accent).ml_px())
                                    .into_any_element()
                            } else if is_mixtape {
                                // Clickable display name for mixtapes
                                div()
                                    .id(SharedString::from("name-display"))
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme.accent))
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.editing_name = true;
                                        cx.notify();
                                    }))
                                    .child(name.clone())
                                    .into_any_element()
                            } else {
                                // Static display name for albums
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(name.clone())
                                    .into_any_element()
                            }),
                    )
                    // Track count / summary
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(if is_mixtape {
                                format!(
                                    "{} tracks, {}",
                                    track_count,
                                    format_duration(total_duration)
                                )
                            } else if included_count == track_count {
                                format!(
                                    "{} tracks, {}",
                                    track_count,
                                    format_duration(total_duration)
                                )
                            } else {
                                format!(
                                    "{} of {} tracks, {}",
                                    included_count,
                                    track_count,
                                    format_duration(total_duration)
                                )
                            }),
                    ),
            )
            // Toolbar
            .child(
                div()
                    .w_full()
                    .px_4()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    // Reset Order button (always available)
                    .child(
                        div()
                            .id(SharedString::from("reset-order-btn"))
                            .px_3()
                            .py_1()
                            .text_sm()
                            .text_color(theme.text)
                            .bg(theme.bg_card)
                            .border_1()
                            .border_color(theme.border)
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_card_hover))
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.reset_order(cx);
                            }))
                            .child("Reset Order"),
                    )
                    // Select All / Deselect All (album mode)
                    .when(!is_mixtape, |el| {
                        el.child(
                            div()
                                .id(SharedString::from("select-all-btn"))
                                .px_3()
                                .py_1()
                                .text_sm()
                                .text_color(theme.text)
                                .bg(theme.bg_card)
                                .border_1()
                                .border_color(theme.border)
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.bg_card_hover))
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    for track in &mut this.tracks {
                                        track.included = true;
                                    }
                                    cx.notify();
                                }))
                                .child("Select All"),
                        )
                        .child(
                            div()
                                .id(SharedString::from("deselect-all-btn"))
                                .px_3()
                                .py_1()
                                .text_sm()
                                .text_color(theme.text)
                                .bg(theme.bg_card)
                                .border_1()
                                .border_color(theme.border)
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.bg_card_hover))
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    for track in &mut this.tracks {
                                        track.included = false;
                                    }
                                    cx.notify();
                                }))
                                .child("Deselect All"),
                        )
                    })
                    // Spacer
                    .child(div().flex_1())
                    // Drop hint (mixtape mode)
                    .when(is_mixtape, |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child("Drop audio files or folders here to add"),
                        )
                    }),
            )
            // Track list (scrollable)
            .child(
                div()
                    .id("track-list-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_scroll()
                    .p_4()
                    .gap_2()
                    .flex()
                    .flex_col()
                    .children(self.track_order.iter().enumerate().map(
                        |(display_index, &track_index)| {
                            if let Some(track) = self.tracks.get(track_index) {
                                self.render_track_row(
                                    display_index,
                                    track_index,
                                    track,
                                    &theme,
                                    &window_title,
                                    cx,
                                )
                                .into_any_element()
                            } else {
                                div().into_any_element()
                            }
                        },
                    )),
            )
            // Footer with Cancel and Done buttons
            .child(
                div()
                    .w_full()
                    .p_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(theme.border)
                    // Changes indicator
                    .child(
                        div()
                            .text_xs()
                            .text_color(if has_changes { theme.warning } else { theme.text_muted })
                            .child(if has_changes { "Unsaved changes" } else { "" }),
                    )
                    // Button row
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            // Cancel button
                            .child(
                                div()
                                    .id(SharedString::from("cancel-btn"))
                                    .px_4()
                                    .py_2()
                                    .text_sm()
                                    .text_color(theme.text)
                                    .bg(theme.bg_card)
                                    .border_1()
                                    .border_color(theme.border)
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_card_hover))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.cancel(window, cx);
                                    }))
                                    .child("Cancel"),
                            )
                            // Done button
                            .child(
                                div()
                                    .id(SharedString::from("done-btn"))
                                    .px_4()
                                    .py_2()
                                    .text_sm()
                                    .text_color(gpui::white())
                                    .bg(theme.accent)
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.success))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.apply_and_close(window, cx);
                                    }))
                                    .child("Done"),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dragged_track_creation() {
        let dragged =
            DraggedTrack::new(0, "Test Track".to_string(), "Mixtape Editor".to_string());
        assert_eq!(dragged.index, 0);
        assert_eq!(dragged.name, "Test Track");
        assert_eq!(dragged.source_window_title, "Mixtape Editor");
    }

    #[test]
    fn test_dragged_track_with_position() {
        let dragged = DraggedTrack::new(1, "Track".to_string(), "Test Window".to_string())
            .with_position(Point {
                x: px(100.),
                y: px(200.),
            });
        assert_eq!(dragged.position.x, px(100.));
        assert_eq!(dragged.position.y, px(200.));
    }
}
