// Profiles module - handles burn profile management

pub mod types;
pub mod storage;

pub use types::{BurnProfile, BurnSettings};
pub use storage::{
    save_profile, load_profile, load_recent_profiles,
    add_to_recent_profiles, remove_from_recent_profiles
};
