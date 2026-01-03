//! Volume Label Dialog
//!
//! Modal dialog for entering the CD volume label with real-time validation.
//! Joliet format: max 16 characters, excludes * / : ; ? \

use gpui::{
    div, prelude::*, px, size, Bounds, Context, FocusHandle, KeyDownEvent,
    Render, SharedString, Window, WindowBounds, WindowOptions,
};

use crate::ui::Theme;

/// Characters not allowed in Joliet volume labels
const EXCLUDED_CHARS: &[char] = &['*', '/', ':', ';', '?', '\\'];
/// Maximum volume label length for Joliet
const MAX_LENGTH: usize = 16;
/// Default volume label
pub const DEFAULT_LABEL: &str = "Untitled MP3CD";

/// Check if a character is valid for Joliet volume labels
fn is_valid_char(c: char) -> bool {
    !c.is_control() && !EXCLUDED_CHARS.contains(&c)
}

/// The Volume Label Dialog modal
pub struct VolumeLabelDialog {
    /// Current text value
    text: String,
    /// Whether we're still showing the default text (to be cleared on first keystroke)
    is_default: bool,
    /// Focus handle for keyboard input
    focus_handle: FocusHandle,
    /// Callback when OK is pressed (sends label to main window)
    on_confirm: Option<Box<dyn Fn(String) + 'static>>,
}

impl VolumeLabelDialog {
    pub fn new(cx: &mut Context<Self>, initial_label: Option<String>) -> Self {
        let text = initial_label.unwrap_or_else(|| DEFAULT_LABEL.to_string());
        let is_default = text == DEFAULT_LABEL;
        Self {
            text,
            is_default,
            focus_handle: cx.focus_handle(),
            on_confirm: None,
        }
    }

    /// Open the Volume Label Dialog window
    ///
    /// The callback will be called with the validated label when OK is pressed.
    /// Returns the window handle.
    pub fn open<F>(cx: &mut gpui::App, initial_label: Option<String>, on_confirm: F) -> gpui::WindowHandle<Self>
    where
        F: Fn(String) + 'static,
    {
        let bounds = Bounds::centered(None, size(px(350.), px(180.)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(350.), px(180.))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("CD Volume Label".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|cx| {
                    let mut dialog = VolumeLabelDialog::new(cx, initial_label);
                    dialog.on_confirm = Some(Box::new(on_confirm));
                    dialog
                })
            },
        )
        .unwrap()
    }

    /// Handle a key press - returns true if the event was handled
    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let keystroke = &event.keystroke;

        // Handle special keys
        if keystroke.key == "backspace" {
            if self.is_default {
                // Clear default text on backspace
                self.text.clear();
                self.is_default = false;
            } else if !self.text.is_empty() {
                self.text.pop();
            }
            cx.notify();
            return true;
        }

        if keystroke.key == "escape" {
            // Close without confirming
            self.cancel(window, cx);
            return true;
        }

        if keystroke.key == "enter" {
            // Confirm the label
            self.confirm(window, cx);
            return true;
        }

        // Handle regular character input
        if let Some(ref key_char) = keystroke.key_char {
            for c in key_char.chars() {
                // Validate character
                if !is_valid_char(c) {
                    continue; // Silently ignore invalid characters
                }

                // Clear default text on first valid keystroke
                if self.is_default {
                    self.text.clear();
                    self.is_default = false;
                }

                // Check length limit
                if self.text.chars().count() >= MAX_LENGTH {
                    continue; // Silently ignore if at max length
                }

                // Add the character
                self.text.push(c);
            }
            cx.notify();
            return true;
        }

        false
    }

    fn confirm(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        // Call the callback with the label
        if let Some(ref on_confirm) = self.on_confirm {
            on_confirm(self.text.clone());
        }
        // Close the dialog window
        window.remove_window();
    }

    fn cancel(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        // Close the dialog window without calling callback
        window.remove_window();
    }
}

impl Render for VolumeLabelDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::from_appearance(window.appearance());
        let text_display = self.text.clone();
        let chars_remaining = MAX_LENGTH.saturating_sub(self.text.chars().count());
        let is_default = self.is_default;

        // Focus the dialog on render
        if !self.focus_handle.is_focused(window) {
            self.focus_handle.focus(window);
        }

        div()
            .key_context("VolumeLabelDialog")
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
            // Description
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child("Enter a name for the CD (max 16 characters):"),
            )
            // Text input display
            .child(
                div()
                    .id(SharedString::from("volume-label-input"))
                    .w_full()
                    .h(px(36.))
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
                            .text_color(if is_default { theme.text_muted } else { theme.text })
                            .when(is_default, |el| el.italic())
                            .child(if text_display.is_empty() {
                                " ".to_string() // Prevent collapse
                            } else {
                                text_display
                            }),
                    )
                    // Cursor
                    .child(
                        div()
                            .w(px(2.))
                            .h(px(20.))
                            .bg(theme.accent)
                            .ml_px(),
                    ),
            )
            // Character count
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(format!("{} characters remaining", chars_remaining)),
            )
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
                            .id(SharedString::from("ok-btn"))
                            .px_4()
                            .py_2()
                            .bg(theme.accent)
                            .text_color(gpui::white())
                            .text_sm()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.success))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm(window, cx);
                            }))
                            .child("OK"),
                    ),
            )
    }
}
