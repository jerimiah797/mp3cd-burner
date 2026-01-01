//! Application state types
//! (Future feature)
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Application-wide settings
#[derive(Debug, Clone, Default)]
pub struct AppSettings {
    /// Whether to simulate burning (don't actually burn)
    pub simulate_burn: bool,
    /// Whether to avoid lossy-to-lossy conversions
    pub no_lossy_conversions: bool,
    /// Whether to embed album art in MP3s
    pub embed_album_art: bool,
}

/// Settings for a burn operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurnSettings {
    /// Target bitrate in kbps
    pub bitrate: u32,
    /// Volume label for the CD
    pub volume_label: String,
}

impl Default for BurnSettings {
    fn default() -> Self {
        Self {
            bitrate: 192,
            volume_label: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_settings_default() {
        let settings = AppSettings::default();
        assert!(!settings.simulate_burn);
        assert!(!settings.no_lossy_conversions);
        assert!(!settings.embed_album_art);
    }

    #[test]
    fn test_burn_settings_default() {
        let settings = BurnSettings::default();
        assert_eq!(settings.bitrate, 192);
        assert!(settings.volume_label.is_empty());
    }
}
