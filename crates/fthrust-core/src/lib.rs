//! Shared on-disk formats and state machines for FTHRust.
#![forbid(unsafe_code)]

mod atomic_json;
mod error;
mod paths;

pub use atomic_json::{read_json, write_json_atomic};
pub use error::{CoreError, Result};
pub use paths::{FthrPaths, runtime_dir};
