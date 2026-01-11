//! Bitrate Override Dialog
//!
//! Modal dialog for manually overriding the calculated MP3 bitrate.
//! Valid range: 64-320 kbps (LAME encoder limits).

use gpui::{
    Bounds, Context, FocusHandle, KeyDownEvent, Render, SharedString, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};

use crate::ui::Theme;

/// Minimum bitrate for LAME encoder
const MIN_BITRATE: u32 = 64;
/// Maximum bitrate for LAME encoder
const MAX_BITRATE: u32 = 320;

/// The Bitrate Override Dialog modal
pub struct BitrateOverrideDialog {
    /// Current input text (numeric string)
    text: String,
    /// The calculated bitrate (for reference display)
    calculated_bitrate: u32,
    /// Focus handle for keyboard input
    focus_handle: FocusHandle,
    /// Callback when Apply/Use Automatic is pressed
    /// Some(bitrate) = set custom bitrate, None = reset to automatic
    on_confirm: Option<Box<dyn Fn(Option<u32>) + 'static>>,
    /// Warning message (e.g., about folders with unavailable source)
    warning_message: Option<String>,
    /// Whether a custom bitrate is currently set (to enable "Use Automatic" button)
    has_custom_bitrate: bool,
}

impl BitrateOverrideDialog {
    pub fn new(
        cx: &mut Context<Self>,
        current_bitrate: u32,
        calculated_bitrate: u32,
        warning_message: Option<String>,
        has_custom_bitrate: bool,
    ) -> Self {
        Self {
            text: current_bitrate.to_string(),
            calculated_bitrate,
            focus_handle: cx.focus_handle(),
            on_confirm: None,
            warning_message,
            has_custom_bitrate,
        }
    }

    /// Open the Bitrate Override Dialog window
    ///
    /// The callback will be called with:
    /// - Some(bitrate) when Apply is pressed
    /// - None when "Use Automatic" is pressed
    /// Returns the window handle.
    #[allow(dead_code)]
    pub fn open<F>(
        cx: &mut gpui::App,
        current_bitrate: u32,
        calculated_bitrate: u32,
        has_custom_bitrate: bool,
        on_confirm: F,
    ) -> gpui::WindowHandle<Self>
    where
        F: Fn(Option<u32>) + 'static,
    {
        Self::open_with_warning(cx, current_bitrate, calculated_bitrate, None, has_custom_bitrate, on_confirm)
    }

    /// Open the Bitrate Override Dialog window with an optional warning
    ///
    /// Use this when there are folders with unavailable source that can't be re-encoded.
    pub fn open_with_warning<F>(
        cx: &mut gpui::App,
        current_bitrate: u32,
        calculated_bitrate: u32,
        warning_message: Option<String>,
        has_custom_bitrate: bool,
        on_confirm: F,
    ) -> gpui::WindowHandle<Self>
    where
        F: Fn(Option<u32>) + 'static,
    {
        // Adjust height if there's a warning or custom bitrate is set
        let height = if warning_message.is_some() || has_custom_bitrate {
            px(260.)
        } else {
            px(200.)
        };
        let bounds = Bounds::centered(None, size(px(320.), height), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(320.), height)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Override Bitrate".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|cx| {
                    let mut dialog = BitrateOverrideDialog::new(
                        cx,
                        current_bitrate,
                        calculated_bitrate,
                        warning_message,
                        has_custom_bitrate,
                    );
                    dialog.on_confirm = Some(Box::new(on_confirm));
                    dialog
                })
            },
        )
        .unwrap()
    }

    /// Parse the current text as a bitrate value
    fn parse_bitrate(&self) -> Option<u32> {
        self.text.parse::<u32>().ok()
    }

    /// Check if the current input is valid
    fn is_valid(&self) -> bool {
        match self.parse_bitrate() {
            Some(br) => (MIN_BITRATE..=MAX_BITRATE).contains(&br),
            None => false,
        }
    }

    /// Handle a key press - returns true if the event was handled
    fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let keystroke = &event.keystroke;

        // Handle special keys
        if keystroke.key == "backspace" {
            if !self.text.is_empty() {
                self.text.pop();
            }
            cx.notify();
            return true;
        }

        if keystroke.key == "escape" {
            self.cancel(window, cx);
            return true;
        }

        if keystroke.key == "enter" {
            if self.is_valid() {
                self.confirm(window, cx);
            }
            return true;
        }

        // Handle digit input only
        if let Some(ref key_char) = keystroke.key_char {
            for c in key_char.chars() {
                // Only allow digits
                if !c.is_ascii_digit() {
                    continue;
                }

                // Limit to reasonable length (max 3 digits for 320)
                if self.text.len() >= 3 {
                    continue;
                }

                self.text.push(c);
            }
            cx.notify();
            return true;
        }

        false
    }

    fn confirm(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(bitrate) = self.parse_bitrate()
            && (MIN_BITRATE..=MAX_BITRATE).contains(&bitrate)
                && let Some(ref on_confirm) = self.on_confirm {
                    on_confirm(Some(bitrate));
                }
        window.remove_window();
    }

    fn use_automatic(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(ref on_confirm) = self.on_confirm {
            on_confirm(None);
        }
        window.remove_window();
    }

    fn cancel(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        window.remove_window();
    }
}

impl Render for BitrateOverrideDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::from_appearance(window.appearance());
        let text_display = self.text.clone();
        let is_valid = self.is_valid();
        let calculated = self.calculated_bitrate;
        let warning = self.warning_message.clone();

        // Focus the dialog on render
        if !self.focus_handle.is_focused(window) {
            self.focus_handle.focus(window);
        }

        div()
            .key_context("BitrateOverrideDialog")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg)
            .p_4()
            .gap_3()
            // Warning message (if any)
            .when_some(warning, |el, msg| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.warning)
                        .p_2()
                        .bg(theme.bg_warning)
                        .rounded_md()
                        .child(format!("⚠️ {}", msg)),
                )
            })
            // Calculated bitrate reference
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(format!("Calculated: {} kbps", calculated)),
            )
            // Input row
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().text_color(theme.text).child("New bitrate:"))
                    .child(
                        div()
                            .id(SharedString::from("bitrate-input"))
                            .w(px(80.))
                            .h(px(36.))
                            .px_3()
                            .flex()
                            .items_center()
                            .bg(theme.bg_card)
                            .border_1()
                            .border_color(if is_valid || self.text.is_empty() {
                                theme.accent
                            } else {
                                theme.danger
                            })
                            .rounded_md()
                            .child(div().text_base().text_color(theme.text).child(
                                if text_display.is_empty() {
                                    " ".to_string()
                                } else {
                                    text_display
                                },
                            ))
                            // Cursor
                            .child(div().w(px(2.)).h(px(20.)).bg(theme.accent).ml_px()),
                    )
                    .child(div().text_sm().text_color(theme.text).child("kbps")),
            )
            // Valid range hint
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(format!("Valid range: {}-{} kbps", MIN_BITRATE, MAX_BITRATE)),
            )
            // "Use Automatic" button (only shown when custom bitrate is set)
            .when(self.has_custom_bitrate, |el| {
                el.child(
                    div()
                        .id(SharedString::from("use-auto-btn"))
                        .w_full()
                        .px_4()
                        .py_2()
                        .bg(theme.bg_card)
                        .text_color(theme.accent)
                        .text_sm()
                        .text_center()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.accent)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_card_hover))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.use_automatic(window, cx);
                        }))
                        .child(format!("Use Automatic ({} kbps)", calculated)),
                )
            })
            // Buttons
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .mt_2()
                    .child(
                        div()
                            .id(SharedString::from("cancel-btn"))
                            .px_4()
                            .py_2()
                            .bg(theme.bg_card)
                            .text_color(theme.text)
                            .text_sm()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.text_muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_card_hover))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel(window, cx);
                            }))
                            .child("Cancel"),
                    )
                    .child(
                        div()
                            .id(SharedString::from("apply-btn"))
                            .px_4()
                            .py_2()
                            .bg(if is_valid {
                                theme.accent
                            } else {
                                theme.bg_card
                            })
                            .text_color(if is_valid {
                                gpui::white()
                            } else {
                                theme.text_muted
                            })
                            .text_sm()
                            .rounded_md()
                            .when(is_valid, |el| el.cursor_pointer())
                            .when(is_valid, |el| el.hover(|s| s.bg(theme.success)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.is_valid() {
                                    this.confirm(window, cx);
                                }
                            }))
                            .child("Apply"),
                    ),
            )
    }
}
