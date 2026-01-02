// Profiles module - handles burn profile management

pub mod manager;
pub mod storage;
pub mod types;

pub use manager::{create_profile, load_profile_from_path, save_profile_to_path, LoadedProfile};
pub use storage::validate_conversion_state;
pub use types::{BurnProfile, BurnSettings, ConversionStateValidation, SavedFolderState};

