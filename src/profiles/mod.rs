// Profiles module - handles burn profile management

pub mod manager;
pub mod storage;
pub mod types;

pub use manager::{prepare_profile_load, save_profile_to_path, ProfileLoadSetup};

