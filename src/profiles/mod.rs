// Profiles module - handles burn profile management

pub mod types;
pub mod storage;

pub use storage::validate_conversion_state;
pub use types::{BurnProfile, BurnSettings, ConversionStateValidation, SavedFolderState};

