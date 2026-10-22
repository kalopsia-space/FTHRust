//! Shared on-disk formats and state machines for FTHRust.
//!
//! Everything the UI, the capture engine and the uploader agree on lives
//! here, byte-compatible with the Python FTHR Clips files under `~/.fthr`:
//!
//! | module | owns |
//! |---|---|
//! | `settings` | `settings.json` (defaults, one-level merge, quarantine, migrations) |
//! | `presets` / `export_presets` | `presets.json`, `export_presets.json` |
//! | `theme` | `theme/theme.json` plus `icons/`, `sounds/`, `fonts/`, zip export/import |
//! | `navigation` | in-app keyboard bindings stored inside `settings.json` |
//! | `hotkeys` | `hotkeys.json`, `hotkeys-portal-cleared.json`, portal trigger names |
//! | `error_codes` | the public `Error NNN` registry |
//! | `clip_files` / `transactional_output` / `media_identity` | clip naming, partial files, staged writes |
//! | `library_index` / `library_ownership` | library scans and linked-folder protection |
//! | `clip_metadata` | `clip_metadata.sqlite3` tags, descriptions and editor drafts |
//! | `audio_manifest` | `<clip>.fthr-audio.json` sidecars and clip renames |
//! | `thumbnails` | `thumbnails/` cache keys, `.meta-v3`, pruning |
//! | `window_geometry` | `main_window_geometry` persistence and restore |
//! | `capture_config` / `save_state` / `capture_health` / `recovery_tracker` | pure capture state machines |
//! | `diagnostics` | `logs/fthr.log`, `logs/fthr-crash.log`, `diagnostics/` sessions |
//! | `paths` / `atomic_json` | the `~/.fthr` path map and atomic JSON writes |
#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod atomic_json;
pub mod audio_manifest;
pub mod capture_config;
pub mod capture_health;
pub mod clip_files;
pub mod clip_metadata;
pub mod clip_readiness;
pub mod diagnostics;
mod error;
pub mod error_codes;
pub mod export_presets;
pub mod game_rules;
pub mod hotkeys;
pub mod library_index;
pub mod library_ownership;
pub mod media_identity;
pub mod navigation;
mod paths;
pub mod presets;
pub mod recovery_tracker;
pub mod save_state;
pub mod screenshot_save;
pub mod settings;
pub mod single_instance;
pub mod theme;
pub mod thumbnails;
pub mod transactional_output;
pub mod window_geometry;

pub use atomic_json::{read_json, write_json_atomic};
pub use error::{CoreError, Result};
pub use paths::{FthrPaths, runtime_dir};
