//! Burning module - ISO creation and CD burning logic
//!
//! This module is framework-agnostic. It uses callbacks/Result types
//! for progress and error reporting instead of Tauri-specific APIs.

pub mod cd;
pub mod coordinator;
pub mod iso;
pub mod iso_manager;
pub mod iso_state;

pub use cd::{burn_iso_with_cancel, check_cd_status, CdStatus};
pub use coordinator::{coordinate_burn, BurnConfig, BurnCoordinationResult};
pub use iso::create_iso;
pub use iso_manager::{
    create_iso_state, generate_iso, spawn_iso_generation, IsoGenerationCheck, IsoGenerationResult,
};
pub use iso_state::{determine_iso_action, IsoAction, IsoState, MAX_ISO_SIZE_BYTES};
