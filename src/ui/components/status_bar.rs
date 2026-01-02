//! Status bar component helpers for displaying stats and progress
//!
//! Provides:
//! - StatusBarState struct for capturing all necessary rendering state
//! - Helper functions for rendering stats and progress displays
//!
//! Button rendering with click handlers remains in folder_list.rs to use cx.listener().

use gpui::{div, prelude::*, SharedString};

use crate::conversion::MultipassEstimate;
use crate::core::{format_duration, BurnStage};
use crate::ui::Theme;

/// State needed to render the status bar
#[derive(Clone)]
pub struct StatusBarState {
    /// Total file count across all folders
    pub total_files: u32,
    /// Total size in bytes
    pub total_size: u64,
    /// Total duration in seconds
    pub total_duration: f64,
    /// Calculated bitrate estimate (if applicable)
    pub bitrate_estimate: Option<MultipassEstimate>,
    /// Whether there are any folders
    pub has_folders: bool,
    /// Whether import is in progress
    pub is_importing: bool,
    /// Import progress (completed, total)
    pub import_progress: (usize, usize),
    /// Whether conversion/burn is in progress
    pub is_converting: bool,
    /// Conversion progress (completed, failed, total)
    pub conversion_progress: (usize, usize, usize),
    /// Current burn stage
    pub burn_stage: BurnStage,
    /// Burn progress percentage (-1 for indeterminate)
    pub burn_progress: i32,
    /// Whether cancellation has been requested
    pub is_cancelled: bool,
    /// Whether a valid ISO exists that can be burned
    pub can_burn_another: bool,
    /// Whether the ISO exceeds CD size limit
    pub iso_exceeds_limit: bool,
    /// ISO size in MB (if available)
    pub iso_size_mb: Option<f64>,
    /// Whether the ISO has been burned at least once
    pub iso_has_been_burned: bool,
}

impl StatusBarState {
    /// Get formatted bitrate display string
    pub fn bitrate_display(&self) -> String {
        match &self.bitrate_estimate {
            Some(e) if e.should_show_bitrate() => format!("{} kbps", e.target_bitrate),
            _ => "--".to_string(),
        }
    }

    /// Get size in MB
    pub fn size_mb(&self) -> f64 {
        self.total_size as f64 / (1024.0 * 1024.0)
    }
}

/// Render the left stats panel (Files, Duration, Size, Target, Bitrate)
pub fn render_stats_panel(state: &StatusBarState, theme: &Theme) -> impl IntoElement {
    let bitrate_display = state.bitrate_display();
    let size_mb = state.size_mb();
    let text_color = theme.text;
    let text_muted = theme.text_muted;
    let success_color = theme.success;

    div()
        .flex()
        .flex_col()
        .gap_1()
        .text_color(text_muted)
        // Row 1: Files and Duration
        .child(
            div()
                .flex()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child("Files:")
                        .child(
                            div()
                                .text_color(text_color)
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(format!("{}", state.total_files)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child("Duration:")
                        .child(
                            div()
                                .text_color(text_color)
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(format_duration(state.total_duration)),
                        ),
                ),
        )
        // Row 2: Size and Target
        .child(
            div()
                .flex()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child("Size:")
                        .child(
                            div()
                                .text_color(text_color)
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(format!("{:.2} MB", size_mb)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child("Target:")
                        .child(
                            div()
                                .text_color(text_color)
                                .font_weight(gpui::FontWeight::BOLD)
                                .child("700 MB"),
                        ),
                ),
        )
        // Row 3: Bitrate and CD-RW indicator
        .child(
            div()
                .flex()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child("Bitrate:")
                        .child(
                            div()
                                .text_color(success_color)
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(bitrate_display),
                        ),
                )
                // CD-RW indicator (only show when erasable disc detected)
                .when(
                    state.is_converting && state.burn_stage == BurnStage::ErasableDiscDetected,
                    |el| {
                        el.child(
                            div()
                                .text_color(theme.danger)
                                .font_weight(gpui::FontWeight::BOLD)
                                .child("CD-RW"),
                        )
                    },
                ),
        )
}

/// Render import progress display
pub fn render_import_progress(state: &StatusBarState, theme: &Theme) -> impl IntoElement {
    let (import_completed, import_total) = state.import_progress;
    let progress_fraction = if import_total > 0 {
        import_completed as f32 / import_total as f32
    } else {
        0.0
    };

    div()
        .id(SharedString::from("import-progress"))
        .w(gpui::px(150.0))
        .h(gpui::px(70.0))
        .rounded_md()
        .border_1()
        .border_color(theme.accent)
        .overflow_hidden()
        .relative()
        // Background progress fill
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .h_full()
                .w(gpui::relative(progress_fraction))
                .bg(theme.accent),
        )
        // Text overlay
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .relative()
                .child(
                    div()
                        .text_lg()
                        .text_color(gpui::white())
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(format!("{}/{}", import_completed, import_total)),
                )
                .child(
                    div()
                        .text_lg()
                        .text_color(gpui::white())
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Importing..."),
                ),
        )
}

/// Calculate progress display values based on burn stage
pub struct ProgressDisplay {
    pub fraction: f32,
    pub text: String,
    pub stage_text: &'static str,
}

impl ProgressDisplay {
    pub fn from_state(state: &StatusBarState) -> Self {
        let (completed, failed, total) = state.conversion_progress;

        match state.burn_stage {
            BurnStage::Converting => {
                let frac = if total > 0 {
                    (completed + failed) as f32 / total as f32
                } else {
                    0.0
                };
                Self {
                    fraction: frac,
                    text: format!("{}/{}", completed + failed, total),
                    stage_text: "Converting...",
                }
            }
            BurnStage::CreatingIso => Self {
                fraction: 1.0,
                text: "".to_string(),
                stage_text: "Creating ISO...",
            },
            BurnStage::WaitingForCd => Self {
                fraction: 1.0,
                text: "".to_string(),
                stage_text: "Insert blank CD",
            },
            BurnStage::ErasableDiscDetected => Self {
                fraction: 1.0,
                text: "".to_string(),
                stage_text: "CD-RW detected",
            },
            BurnStage::Erasing => {
                let frac = if state.burn_progress >= 0 {
                    state.burn_progress as f32 / 100.0
                } else {
                    0.0
                };
                let text = if state.burn_progress >= 0 {
                    format!("{}%", state.burn_progress)
                } else {
                    "".to_string()
                };
                Self {
                    fraction: frac,
                    text,
                    stage_text: "Erasing...",
                }
            }
            BurnStage::Burning => {
                let frac = if state.burn_progress >= 0 {
                    state.burn_progress as f32 / 100.0
                } else {
                    0.0
                };
                let text = if state.burn_progress >= 0 {
                    format!("{}%", state.burn_progress)
                } else {
                    "".to_string()
                };
                Self {
                    fraction: frac,
                    text,
                    stage_text: "Burning...",
                }
            }
            BurnStage::Finishing => Self {
                fraction: 1.0,
                text: "".to_string(),
                stage_text: "Finishing...",
            },
            BurnStage::Complete => Self {
                fraction: 1.0,
                text: "✓".to_string(),
                stage_text: "Complete!",
            },
            BurnStage::Cancelled => Self {
                fraction: 0.0,
                text: "".to_string(),
                stage_text: "Cancelled",
            },
        }
    }
}

/// Get the stage color based on current state
pub fn get_stage_color(state: &StatusBarState, theme: &Theme) -> gpui::Hsla {
    match state.burn_stage {
        BurnStage::Cancelled => theme.danger,
        BurnStage::Complete => theme.success,
        _ if state.is_cancelled => theme.danger,
        _ => theme.success,
    }
}

/// Check if the current stage is cancelable
pub fn is_stage_cancelable(state: &StatusBarState) -> bool {
    state.burn_stage != BurnStage::Complete
        && state.burn_stage != BurnStage::Cancelled
        && !state.is_cancelled
}

/// Render "Too Large" warning when ISO exceeds CD limit
pub fn render_iso_too_large(iso_size_mb: f64, theme: &Theme) -> impl IntoElement {
    let size_text = format!("{:.0} MB\nToo Large!", iso_size_mb);

    div()
        .id(SharedString::from("iso-too-large-btn"))
        .w(gpui::px(150.0))
        .h(gpui::px(70.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.danger)
        .text_color(gpui::white())
        .text_sm()
        .rounded_md()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_center()
        .child(size_text)
}
