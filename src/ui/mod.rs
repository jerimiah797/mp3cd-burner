//! UI module - GPUI views and components
//!
//! This module contains all UI-related code:
//! - `components/` - Reusable UI components (FolderItem, DropZone, etc.)
//! - `theme` - OS-aware light and dark mode color schemes

pub mod components;
pub mod theme;

pub use theme::Theme;
