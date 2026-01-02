//! Display Settings modal
//!
//! Controls which details are shown for each folder in the folder list.

use gpui::{
    div, prelude::*, px, size, Bounds, Context, Render, SharedString, Window,
    WindowBounds, WindowHandle, WindowOptions,
};

use crate::core::DisplaySettings;
use crate::ui::Theme;

/// The Display Settings modal
pub struct DisplaySettingsModal {
    show_file_count: bool,
    show_original_size: bool,
    show_converted_size: bool,
    show_source_format: bool,
    show_source_bitrate: bool,
    show_final_bitrate: bool,
}

impl DisplaySettingsModal {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Read current settings from global
        let settings = cx.global::<DisplaySettings>();
        Self {
            show_file_count: settings.show_file_count,
            show_original_size: settings.show_original_size,
            show_converted_size: settings.show_converted_size,
            show_source_format: settings.show_source_format,
            show_source_bitrate: settings.show_source_bitrate,
            show_final_bitrate: settings.show_final_bitrate,
        }
    }

    /// Open the Display Settings window
    pub fn open(cx: &mut gpui::App) -> WindowHandle<Self> {
        let bounds = Bounds::centered(None, size(px(320.), px(380.)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(320.), px(380.))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Display Settings".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| DisplaySettingsModal::new(cx)),
        )
        .unwrap()
    }

    fn toggle_file_count(&mut self, cx: &mut Context<Self>) {
        self.show_file_count = !self.show_file_count;
        self.save_settings(cx);
        cx.notify();
    }

    fn toggle_original_size(&mut self, cx: &mut Context<Self>) {
        self.show_original_size = !self.show_original_size;
        self.save_settings(cx);
        cx.notify();
    }

    fn toggle_converted_size(&mut self, cx: &mut Context<Self>) {
        self.show_converted_size = !self.show_converted_size;
        self.save_settings(cx);
        cx.notify();
    }

    fn toggle_source_format(&mut self, cx: &mut Context<Self>) {
        self.show_source_format = !self.show_source_format;
        self.save_settings(cx);
        cx.notify();
    }

    fn toggle_source_bitrate(&mut self, cx: &mut Context<Self>) {
        self.show_source_bitrate = !self.show_source_bitrate;
        self.save_settings(cx);
        cx.notify();
    }

    fn toggle_final_bitrate(&mut self, cx: &mut Context<Self>) {
        self.show_final_bitrate = !self.show_final_bitrate;
        self.save_settings(cx);
        cx.notify();
    }

    fn save_settings(&self, cx: &mut Context<Self>) {
        let settings = cx.global_mut::<DisplaySettings>();
        settings.show_file_count = self.show_file_count;
        settings.show_original_size = self.show_original_size;
        settings.show_converted_size = self.show_converted_size;
        settings.show_source_format = self.show_source_format;
        settings.show_source_bitrate = self.show_source_bitrate;
        settings.show_final_bitrate = self.show_final_bitrate;

        // Persist to disk
        if let Err(e) = settings.save() {
            eprintln!("Failed to save display settings: {}", e);
        }
    }

    fn render_checkbox(
        &self,
        id: &str,
        label: &str,
        hint: &str,
        checked: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let checkbox_icon = if checked { "☑" } else { "☐" };
        let accent = theme.accent;
        let text_muted = theme.text_muted;
        let text_color = theme.text;
        let bg_hover = theme.bg_card_hover;
        // Convert to owned strings to satisfy 'static requirement
        let label = label.to_string();
        let hint = hint.to_string();

        div()
            .id(SharedString::from(id.to_string()))
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(bg_hover))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                on_click(this, cx);
            }))
            .child(
                div()
                    .text_xl()
                    .text_color(if checked { accent } else { text_muted })
                    .child(checkbox_icon),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_px()
                    .child(
                        div()
                            .text_sm()
                            .text_color(text_color)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(text_muted)
                            .child(hint),
                    ),
            )
    }
}

impl Render for DisplaySettingsModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::from_appearance(window.appearance());

        // Capture current state for closures
        let show_file_count = self.show_file_count;
        let show_original_size = self.show_original_size;
        let show_converted_size = self.show_converted_size;
        let show_source_format = self.show_source_format;
        let show_source_bitrate = self.show_source_bitrate;
        let show_final_bitrate = self.show_final_bitrate;

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg)
            .p_4()
            .gap_2()
            // Title
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child("Folder Item Display"),
            )
            // Description
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .pb_2()
                    .child("Choose which details to show for each folder:"),
            )
            // Checkboxes
            .child(self.render_checkbox(
                "show-source-format",
                "Show Source Format",
                "e.g., \"FLAC\" or \"MP3/AAC\"",
                show_source_format,
                &theme,
                cx,
                |this, cx| this.toggle_source_format(cx),
            ))
            .child(self.render_checkbox(
                "show-source-bitrate",
                "Show Source Bitrate",
                "e.g., \"320k\" or \"128-320k\"",
                show_source_bitrate,
                &theme,
                cx,
                |this, cx| this.toggle_source_bitrate(cx),
            ))
            .child(self.render_checkbox(
                "show-file-count",
                "Show File Count",
                "e.g., \"12 files\"",
                show_file_count,
                &theme,
                cx,
                |this, cx| this.toggle_file_count(cx),
            ))
            .child(self.render_checkbox(
                "show-original-size",
                "Show Original Size",
                "e.g., \"500 MB\"",
                show_original_size,
                &theme,
                cx,
                |this, cx| this.toggle_original_size(cx),
            ))
            .child(self.render_checkbox(
                "show-converted-size",
                "Show Converted Size",
                "e.g., \"→ 180 MB\"",
                show_converted_size,
                &theme,
                cx,
                |this, cx| this.toggle_converted_size(cx),
            ))
            .child(self.render_checkbox(
                "show-final-bitrate",
                "Show Final Bitrate",
                "e.g., \"@192k\"",
                show_final_bitrate,
                &theme,
                cx,
                |this, cx| this.toggle_final_bitrate(cx),
            ))
    }
}
