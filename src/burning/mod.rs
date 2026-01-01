//! Burning module - ISO creation and CD burning logic
//!
//! This module is framework-agnostic. It uses callbacks/Result types
//! for progress and error reporting instead of Tauri-specific APIs.

pub mod cd;
pub mod iso;

