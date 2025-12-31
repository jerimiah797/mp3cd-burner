//! Theme module - OS-aware light and dark mode color schemes

use gpui::{rgb, Hsla, WindowAppearance};

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
