//! Theme module - OS-aware light and dark mode color schemes

use gpui::{Hsla, WindowAppearance, rgb};

/// Color scheme for the application
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Main window background
    pub bg: Hsla,
    /// Card/container background
    pub bg_card: Hsla,
    /// Card background on hover
    pub bg_card_hover: Hsla,
    /// Primary text color
    pub text: Hsla,
    /// Secondary/muted text color
    pub text_muted: Hsla,
    /// Border color
    pub border: Hsla,
    /// Accent color (for highlights, drop targets)
    pub accent: Hsla,
    /// Success/action button color (green)
    pub success: Hsla,
    /// Success button hover color
    pub success_hover: Hsla,
    /// Danger/remove color (red)
    pub danger: Hsla,
    /// Background for folders queued for transcoding
    pub bg_queued: Hsla,
    /// Hover background for folders queued for transcoding
    pub bg_queued_hover: Hsla,
    /// Progress line color (brighter than queued background)
    pub progress_line: Hsla,
}

impl Theme {
    /// Dark mode color scheme (matches the original Tauri app)
    pub fn dark() -> Self {
        Self {
            bg: rgb(0x1e1e1e).into(),
            bg_card: rgb(0x2d2d2d).into(),
            bg_card_hover: rgb(0x3d3d3d).into(),
            text: rgb(0xffffff).into(),
            text_muted: rgb(0x9ca3af).into(),
            border: rgb(0x404040).into(),
            accent: rgb(0x3b82f6).into(),
            success: rgb(0x22c55e).into(),
            success_hover: rgb(0x16a34a).into(),
            danger: rgb(0xef4444).into(),
            bg_queued: rgb(0x3a2525).into(),       // Dark red tint
            bg_queued_hover: rgb(0x452a2a).into(), // Slightly lighter
            progress_line: rgb(0x6b3a3a).into(),   // Brighter red for progress
        }
    }

    /// Light mode color scheme
    pub fn light() -> Self {
        Self {
            bg: rgb(0xf5f5f5).into(),
            bg_card: rgb(0xffffff).into(),
            bg_card_hover: rgb(0xf8fafc).into(),
            text: rgb(0x1e293b).into(),
            text_muted: rgb(0x64748b).into(),
            border: rgb(0xe2e8f0).into(),
            accent: rgb(0x3b82f6).into(),
            success: rgb(0x22c55e).into(),
            success_hover: rgb(0x16a34a).into(),
            danger: rgb(0xef4444).into(),
            bg_queued: rgb(0xfce8e8).into(), // Light pink/red tint
            bg_queued_hover: rgb(0xf8d4d4).into(), // Slightly darker on hover
            progress_line: rgb(0xe57373).into(), // Brighter red for progress
        }
    }

    /// Get the appropriate theme based on window appearance
    pub fn from_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }
}
