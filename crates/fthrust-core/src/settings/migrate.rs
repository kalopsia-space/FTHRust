//! Settings migrations (filled in by a later task).

use crate::FthrPaths;
use serde_json::{Map, Value};

/// Applies settings migrations in place; returns true when anything changed.
pub(crate) fn migrate(
    _data: &mut Map<String, Value>,
    _paths: &FthrPaths,
    _file_existed: bool,
) -> bool {
    false
}
