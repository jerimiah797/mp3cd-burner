//! FolderList component - The main application view with folder list
//!
//! This is currently the root view of the application, containing:
//! - Header
//! - Folder list with drag-and-drop
//! - Status bar

mod conversion;
mod encoder;
mod folders;
mod iso;
mod profiles;
mod render;
#[cfg(test)]
mod tests;

use gpui::{Context, FocusHandle, ScrollHandle};
use std::path::PathBuf;

use crate::burning::IsoState;
use crate::conversion::{BackgroundEncoderHandle, OutputManager};
use crate::core::{ConversionState, ImportState, MusicFolder};
use crate::profiles::ProfileLoadSetup;

pub(crate) use super::VolumeLabelDialog;

/// The main folder list view
///
/// Handles:
/// - Displaying the list of folders
/// - External drag-drop from Finder (ExternalPaths)
/// - Internal drag-drop for reordering
/// - Empty state rendering
pub struct FolderList {
    /// The list of scanned music folders
    pub(crate) folders: Vec<MusicFolder>,
    /// Currently hovered drop target index (for visual feedback)
    pub(crate) drop_target_index: Option<usize>,
    /// Whether we've subscribed to appearance changes
    pub(crate) appearance_subscription_set: bool,
    /// Whether we've subscribed to bounds changes (for saving window state)
    pub(crate) bounds_subscription_set: bool,
    /// Handle for scroll state
    pub(crate) scroll_handle: ScrollHandle,
    /// Conversion progress state
    pub(crate) conversion_state: ConversionState,
    /// Import progress state
    pub(crate) import_state: ImportState,
    /// Focus handle for receiving actions (None in tests)
    pub(crate) focus_handle: Option<FocusHandle>,
    /// Background encoder handle for immediate conversion (None until initialized)
    pub(crate) background_encoder: Option<BackgroundEncoderHandle>,
    /// Event receiver for background encoder progress updates (std::sync::mpsc for easy polling)
    pub(crate) encoder_event_rx: Option<std::sync::mpsc::Receiver<crate::conversion::EncoderEvent>>,
    /// Output manager for session-based directories (None until initialized)
    pub(crate) output_manager: Option<OutputManager>,
    /// Current ISO state (for "Burn Another" functionality)
    pub(crate) iso_state: Option<IsoState>,
    /// Whether auto-ISO generation has been attempted (prevents retry loop on failure)
    pub(crate) iso_generation_attempted: bool,
    /// Whether the current ISO has been burned at least once (for "Burn Another" vs "Burn")
    pub(crate) iso_has_been_burned: bool,
    /// Timestamp of last folder list change (for debounced bitrate recalculation)
    pub(crate) last_folder_change: Option<std::time::Instant>,
    /// Last calculated bitrate (to detect changes that require re-encoding)
    pub(crate) last_calculated_bitrate: Option<u32>,
    /// Whether we need to grab initial focus (for menu items to work)
    pub(crate) needs_initial_focus: bool,
    /// Flag to clear folders after save completes (for New -> Save flow)
    pub(crate) pending_new_after_save: bool,
    /// Flag to show open file picker after save completes (for Open -> Save flow)
    pub(crate) pending_open_after_save: bool,
    /// Pending profile load setup (for async profile loading)
    pub(crate) pending_profile_load: Option<ProfileLoadSetup>,
    /// CD volume label (for ISO creation)
    pub(crate) volume_label: String,
    /// Receiver for volume label updates from the dialog
    pub(crate) pending_volume_label_rx: Option<std::sync::mpsc::Receiver<String>>,
    /// Pending burn action to trigger after volume label dialog closes
    pub(crate) pending_burn_action: Option<PendingBurnAction>,
    /// Path to the currently saved profile (None if never saved)
    pub(crate) current_profile_path: Option<PathBuf>,
    /// Whether there are unsaved changes since last save/load
    pub(crate) has_unsaved_changes: bool,
}

/// Action to take after volume label dialog closes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingBurnAction {
    /// Burn existing ISO
    BurnExisting,
    /// Run conversion then burn
    ConvertAndBurn,
}

impl FolderList {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
            appearance_subscription_set: false,
            bounds_subscription_set: false,
            scroll_handle: ScrollHandle::new(),
            conversion_state: ConversionState::new(),
            import_state: ImportState::new(),
            focus_handle: Some(cx.focus_handle()),
            background_encoder: None,
            encoder_event_rx: None,
            output_manager: None,
            iso_state: None,
            iso_generation_attempted: false,
            iso_has_been_burned: false,
            last_folder_change: None,
            last_calculated_bitrate: None,
            needs_initial_focus: true,
            pending_new_after_save: false,
            pending_open_after_save: false,
            pending_profile_load: None,
            volume_label: "Untitled MP3CD".to_string(),
            pending_volume_label_rx: None,
            pending_burn_action: None,
            current_profile_path: None,
            has_unsaved_changes: false,
        }
    }

    /// Create a new FolderList for testing (without GPUI context)
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            folders: Vec::new(),
            drop_target_index: None,
            appearance_subscription_set: false,
            bounds_subscription_set: false,
            scroll_handle: ScrollHandle::new(),
            conversion_state: ConversionState::new(),
            import_state: ImportState::new(),
            focus_handle: None,
            background_encoder: None,
            encoder_event_rx: None,
            output_manager: None,
            iso_state: None,
            iso_generation_attempted: false,
            iso_has_been_burned: false,
            last_folder_change: None,
            last_calculated_bitrate: None,
            needs_initial_focus: false,
            pending_new_after_save: false,
            pending_open_after_save: false,
            pending_profile_load: None,
            volume_label: "Untitled MP3CD".to_string(),
            pending_volume_label_rx: None,
            pending_burn_action: None,
            current_profile_path: None,
            has_unsaved_changes: false,
        }
    }
}
