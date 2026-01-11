//! About window component

use gpui::{
    Bounds, Context, Render, SharedString, Window, WindowBounds, WindowHandle, WindowOptions, div,
    img, prelude::*, px, size,
};
use std::path::{Path, PathBuf};

use crate::ui::Theme;

const GITHUB_REPO_URL: &str = "https://github.com/jerimiah797/mp3cd-burner";
const GITHUB_SPONSOR_URL: &str = "https://github.com/sponsors/jerimiah797";

/// Open a URL in the default browser
fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// The About window content
pub struct AboutBox;

impl AboutBox {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }

    /// Open the About window
    pub fn open(cx: &mut gpui::App) -> WindowHandle<Self> {
        let bounds = Bounds::centered(None, size(px(420.), px(280.)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(420.), px(280.))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("About MP3 CD Burner".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(AboutBox::new),
        )
        .unwrap()
    }
}

impl Render for AboutBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let version = env!("CARGO_PKG_VERSION");
        let theme = Theme::from_appearance(window.appearance());
        let link_color = theme.accent;

        // Path to icon - use PNG for GPUI compatibility
        // Try development path first (works with cargo run), fallback to bundle path
        let dev_path = concat!(env!("CARGO_MANIFEST_DIR"), "/macos/icon_128.png");
        let icon_path: Option<PathBuf> = if Path::new(dev_path).exists() {
            Some(PathBuf::from(dev_path))
        } else if let Ok(exe_path) = std::env::current_exe() {
            // Release: icon is in the app bundle's Resources folder
            // exe is at Contents/MacOS/MP3-CD-Burner, so go up to Contents then into Resources
            exe_path
                .parent() // MacOS
                .and_then(|p| p.parent()) // Contents
                .map(|p| p.join("Resources").join("icon_128.png"))
                .filter(|p| p.exists())
        } else {
            None
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .size_full()
            .bg(theme.bg)
            .p_4()
            .gap_4()
            .child(
                // Icon on the left
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(100.))
                    .h(px(100.))
                    .when_some(icon_path, |el, path| {
                        el.child(
                            img(path.as_path())
                                .size_full()
                                .object_fit(gpui::ObjectFit::Contain),
                        )
                    }),
            )
            .child(
                // Text on the right
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        // App name
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.text)
                            .child("MP3 CD Burner"),
                    )
                    .child(
                        // Version
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!("Version {}", version))),
                    )
                    .child(
                        // Spacer
                        div().h(px(8.)),
                    )
                    .child(
                        // Description
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child("Convert and burn music to MP3 CDs"),
                    )
                    .child(
                        // Spacer
                        div().h(px(8.)),
                    )
                    .child(
                        // Built with
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child("Built with Rust and GPUI"),
                    )
                    .child(
                        // FFmpeg acknowledgment
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child("Powered by FFmpeg (ffmpeg.org)"),
                    )
                    .child(
                        // Copyright
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child("© 2026 Jerimiah Ham"),
                    )
                    .child(
                        // Spacer
                        div().h(px(12.)),
                    )
                    .child(
                        // Open source & support links
                        div()
                            .flex()
                            .flex_row()
                            .gap_3()
                            .child(
                                div()
                                    .id("github-link")
                                    .text_xs()
                                    .text_color(link_color)
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    .on_click(cx.listener(|_, _, _, _| {
                                        open_url(GITHUB_REPO_URL);
                                    }))
                                    .child("View on GitHub"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child("·"),
                            )
                            .child(
                                div()
                                    .id("sponsor-link")
                                    .text_xs()
                                    .text_color(link_color)
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    .on_click(cx.listener(|_, _, _, _| {
                                        open_url(GITHUB_SPONSOR_URL);
                                    }))
                                    .child("Support this project"),
                            ),
                    ),
            )
    }
}
