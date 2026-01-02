// Profiles module - handles burn profile management

pub mod manager;
pub mod storage;
pub mod types;

pub use manager::{load_profile_from_path, save_profile_to_path};

