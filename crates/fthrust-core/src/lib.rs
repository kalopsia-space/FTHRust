//! Shared on-disk formats and state machines for FTHRust.
#![forbid(unsafe_code)]

mod atomic_json;
pub mod audio_manifest;
pub mod clip_files;
pub mod clip_metadata;
mod error;
pub mod error_codes;
pub mod export_presets;
pub mod hotkeys;
pub mod library_index;
pub mod library_ownership;
pub mod media_identity;
pub mod navigation;
mod paths;
pub mod presets;
pub mod settings;
pub mod theme;
pub mod thumbnails;
pub mod transactional_output;
pub mod window_geometry;

pub use atomic_json::{read_json, write_json_atomic};
pub use error::{CoreError, Result};
pub use paths::{FthrPaths, runtime_dir};
