//! StatusBar component - Bottom status bar with folder count and action button

use gpui::{div, prelude::*, rgb, Context, IntoElement, SharedString};

/// Properties for the status bar
pub struct StatusBarProps {
    pub folder_count: usize,
    pub button_label: &'static str,
    pub button_enabled: bool,
}

/// Render the status bar
///
/// Displays folder count on the left and an action button on the right.
pub fn render_status_bar<V: 'static>(
    props: StatusBarProps,
    cx: &mut Context<V>,
    on_button_click: impl Fn(&mut V) + 'static,
) -> impl IntoElement {
    let StatusBarProps {
        folder_count,
        button_label,
        button_enabled,
    } = props;

    let folder_text = if folder_count == 1 {
        "1 folder".to_string()
    } else {
        format!("{} folders", folder_count)
    };

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
                .id(SharedString::from("action-button"))
                .px_3()
                .py_1()
                .bg(if button_enabled {
                    rgb(0x3b82f6)
                } else {
                    rgb(0x94a3b8)
                })
                .text_color(gpui::white())
                .rounded_md()
                .when(button_enabled, |el| {
                    el.cursor_pointer().hover(|s| s.bg(rgb(0x2563eb)))
                })
                .on_click(cx.listener(move |view, _event, _window, _cx| {
                    if button_enabled {
                        on_button_click(view);
                    }
                }))
                .child(button_label),
        )
}
