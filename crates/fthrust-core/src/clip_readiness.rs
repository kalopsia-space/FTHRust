//! Per-clip readiness (`core/clip_readiness.py`): a saved clip is committed
//! by the engine, may still be finalising (audio mux, overlays), and only
//! then becomes accessible to the library, the editor and uploads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Where a clip is on its way to being usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessState {
    /// The engine wrote the base clip; post-processing has not started.
    EngineCommitted,
    /// Post-processing is running.
    Finalizing,
    /// Usable.
    Ready,
    /// Usable, with warnings from optional steps.
    ReadyWithWarning,
    /// Not usable.
    FinalizationFailed,
}

#[derive(Clone, Debug)]
struct Handle {
    state: ReadinessState,
    warnings: Vec<String>,
}

/// Tracks readiness per path. Untracked paths are `Ready` when the file
/// exists and `FinalizationFailed` otherwise.
#[derive(Debug, Default)]
pub struct ClipReadinessRegistry {
    handles: HashMap<PathBuf, Handle>,
    /// How many times [`Self::state`] was asked (each one canonicalises and
    /// may stat): diagnostics and tests.
    state_lookups: AtomicU64,
}

impl Clone for ClipReadinessRegistry {
    fn clone(&self) -> Self {
        Self {
            handles: self.handles.clone(),
            state_lookups: AtomicU64::new(self.state_lookups()),
        }
    }
}

fn key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|d| d.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
}

impl ClipReadinessRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// The engine committed `path`; `needs_finalization` keeps it
    /// inaccessible until [`Self::ready`], [`Self::complete`] or
    /// [`Self::finalization_failed`]. Returns the new state.
    pub fn engine_committed(&mut self, path: &Path, needs_finalization: bool) -> ReadinessState {
        let state = if needs_finalization {
            ReadinessState::Finalizing
        } else {
            ReadinessState::Ready
        };
        self.handles.insert(
            key(path),
            Handle {
                state,
                warnings: Vec::new(),
            },
        );
        state
    }

    fn get_or_create(&mut self, path: &Path) -> &mut Handle {
        let k = key(path);
        let exists = path.is_file();
        self.handles.entry(k).or_insert_with(|| Handle {
            state: if exists {
                ReadinessState::Ready
            } else {
                ReadinessState::EngineCommitted
            },
            warnings: Vec::new(),
        })
    }

    /// Marks `path` ready.
    pub fn ready(&mut self, path: &Path) {
        self.get_or_create(path).state = ReadinessState::Ready;
    }

    /// Records a warning from an optional step.
    pub fn record_warning(&mut self, path: &Path, message: &str) {
        self.get_or_create(path).warnings.push(message.to_owned());
    }

    /// Finalisation finished: ready, with a warning when any was recorded.
    pub fn complete(&mut self, path: &Path) {
        let h = self.get_or_create(path);
        h.state = if h.warnings.is_empty() {
            ReadinessState::Ready
        } else {
            ReadinessState::ReadyWithWarning
        };
    }

    /// A finalisation step failed; the base clip stays usable when
    /// `base_clip_usable`.
    pub fn finalization_failed(&mut self, path: &Path, message: &str, base_clip_usable: bool) {
        let h = self.get_or_create(path);
        h.warnings.push(message.to_owned());
        h.state = if base_clip_usable {
            ReadinessState::ReadyWithWarning
        } else {
            ReadinessState::FinalizationFailed
        };
    }

    /// Whether `path` has an entry.
    pub fn is_tracked(&self, path: &Path) -> bool {
        self.handles.contains_key(&key(path))
    }

    /// How many times [`Self::state`] (and so [`Self::can_access`]) was asked.
    /// A page that asks per card and frame shows here.
    pub fn state_lookups(&self) -> u64 {
        self.state_lookups.load(Ordering::Relaxed)
    }

    /// The state of `path`.
    pub fn state(&self, path: &Path) -> ReadinessState {
        self.state_lookups.fetch_add(1, Ordering::Relaxed);
        match self.handles.get(&key(path)) {
            Some(h) => h.state,
            None if path.is_file() => ReadinessState::Ready,
            None => ReadinessState::FinalizationFailed,
        }
    }

    /// Whether the clip may be opened, listed or uploaded.
    pub fn can_access(&self, path: &Path) -> bool {
        matches!(
            self.state(path),
            ReadinessState::Ready | ReadinessState::ReadyWithWarning
        )
    }

    /// Whether any tracked clip is not usable yet (committed, finalising or
    /// failed). While none is, a listed clip is accessible without asking
    /// the file system about it: [`Self::can_access`] canonicalises and
    /// stats, which a page of cards cannot afford every frame.
    pub fn has_blocked(&self) -> bool {
        self.handles.values().any(|h| {
            !matches!(
                h.state,
                ReadinessState::Ready | ReadinessState::ReadyWithWarning
            )
        })
    }

    /// Warnings recorded for `path`.
    pub fn warnings(&self, path: &Path) -> Vec<String> {
        self.handles
            .get(&key(path))
            .map(|h| h.warnings.clone())
            .unwrap_or_default()
    }

    /// Drops the entry for `path`.
    pub fn forget(&mut self, path: &Path) {
        self.handles.remove(&key(path));
    }
}
