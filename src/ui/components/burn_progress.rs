//! Burn Progress Window
//!
//! A minimal window shown when the main window is closed during a burn.
//! Shows burn progress and quits the app when complete.

use std::time::Duration;

use gpui::{
    div, px, size, App, AppContext, AsyncApp, Context, IntoElement, ParentElement, Render, Styled,
    Timer, WeakEntity, Window, WindowOptions,
};

use crate::core::{BurnStage, ConversionState};

/// Minimal burn progress window
pub struct BurnProgressWindow {
    conversion_state: ConversionState,
}

impl BurnProgressWindow {
    /// Open a burn progress window
    pub fn open(cx: &mut App, conversion_state: ConversionState) {
        let options = WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds {
                origin: gpui::point(px(100.0), px(100.0)),
                size: size(px(300.0), px(100.0)),
            })),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Burn in Progress".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            window_min_size: Some(size(px(300.0), px(100.0))),
            ..Default::default()
        };

        let state_for_window = conversion_state.clone();

        let _ = cx.open_window(options, |_window, cx| {
            cx.new(|cx| {
                let window = BurnProgressWindow {
                    conversion_state: state_for_window,
                };
                // Start polling for completion
                window.start_polling(cx);
                window
            })
        });
    }

    fn start_polling(&self, cx: &mut Context<Self>) {
        let state = self.conversion_state.clone();

        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                loop {
                    Timer::after(Duration::from_millis(100)).await;

                    // Check if burn is complete
                    if !state.is_converting() {
                        // Burn finished - quit app
                        let _ = async_cx.update(|cx| {
                            cx.quit();
                        });
                        break;
                    }

                    // Refresh UI
                    let _ = this.update(&mut async_cx, |_, cx| {
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn stage_text(&self) -> &'static str {
        match self.conversion_state.get_stage() {
            BurnStage::Converting => "Converting files...",
            BurnStage::CreatingIso => "Creating ISO...",
            BurnStage::WaitingForCd => "Insert blank CD...",
            BurnStage::Erasing => "Erasing disc...",
            BurnStage::Burning => "Burning CD...",
            BurnStage::Finishing => "Finishing...",
            BurnStage::ErasableDiscDetected => "Erasable disc detected...",
            BurnStage::Complete => "Complete!",
            BurnStage::Cancelled => "Cancelled",
        }
    }
}

impl Render for BurnProgressWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let stage_text = self.stage_text();
        let progress = self.conversion_state.get_burn_progress();

        let progress_text = if progress >= 0 {
            format!("{}%", progress)
        } else {
            "...".to_string()
        };

        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .size_full()
            .bg(gpui::rgb(0x2d2d2d))
            .text_color(gpui::rgb(0xffffff))
            .child(
                div()
                    .text_size(px(14.0))
                    .child(stage_text),
            )
            .child(
                div()
                    .text_size(px(24.0))
                    .mt(px(8.0))
                    .child(progress_text),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .mt(px(12.0))
                    .text_color(gpui::rgb(0x888888))
                    .child("Do not close this window"),
            )
    }
}
