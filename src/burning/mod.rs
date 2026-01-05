//! Burning module - ISO creation and CD burning logic
//!
//! This module is framework-agnostic. It uses callbacks/Result types
//! for progress and error reporting instead of Tauri-specific APIs.

pub mod cd;
pub mod coordinator;
pub mod iso;
pub mod iso_manager;
pub mod iso_state;
pub mod workflow;

pub use iso_manager::{IsoGenerationCheck, spawn_iso_generation};
pub use iso_state::{IsoAction, IsoState, determine_iso_action};
pub use workflow::{execute_burn_existing, execute_full_burn};
