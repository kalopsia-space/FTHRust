//! Same-directory staging for atomically published media outputs.

use crate::error::{CoreError, Result};
use std::path::{Path, PathBuf};

/// A hidden staging path next to a final output path.
#[derive(Debug)]
pub struct StagedOutput {
    final_path: PathBuf,
    staged: PathBuf,
}

impl StagedOutput {
    /// Choose a staging name `.<stem>.partial.<uuid><suffix>` beside
    /// `final_path`. Does not touch the filesystem.
    pub fn new(final_path: &Path) -> Self {
        let stem = final_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let suffix = final_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let token = uuid::Uuid::new_v4().simple();
        let name = format!(".{stem}.partial.{token}{suffix}");
        Self {
            final_path: final_path.to_path_buf(),
            staged: final_path.with_file_name(name),
        }
    }

    /// The staging path writers should produce.
    pub fn staged_path(&self) -> &Path {
        &self.staged
    }

    /// Rename the staged file over the final path (overwriting) and return it.
    pub fn commit(self) -> Result<PathBuf> {
        if self.staged.parent() != self.final_path.parent() {
            return Err(CoreError::Invalid(
                "staged and final output must be in the same directory".into(),
            ));
        }
        std::fs::rename(&self.staged, &self.final_path)?;
        Ok(self.final_path)
    }

    /// Remove the staged file; a missing file is not an error.
    pub fn discard(self) {
        if let Err(e) = std::fs::remove_file(&self.staged)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.staged.display(), error = %e, "failed to discard staged output");
        }
    }
}
