//! Core application logic and state
//!
//! This module contains:
//! - Application-wide state (settings, preferences)
//! - Actions that can be triggered from menus or UI

mod state;

pub use state::{AppSettings, BurnSettings};
