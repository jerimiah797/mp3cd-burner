//! Header component - Application title bar

use gpui::{div, prelude::*, rgb, IntoElement};

/// Render the application header
pub struct Header;

impl Header {
    /// Render the header with the given title
    pub fn render(title: &str) -> impl IntoElement {
        div()
            .w_full()
            .h_12()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x1e293b))
            .text_color(gpui::white())
            .text_lg()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(title.to_string())
    }
}
