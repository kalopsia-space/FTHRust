//! Focus pause (`FocusMonitor` in `main.py`, on the compositor's toplevel
//! list): the activated toplevel's title contains the target window name
//! case-insensitively, or the capture is paused.

use fthrust_wayland::toplevels::Toplevel;

/// A focus transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusChange {
    /// The game lost focus.
    Lost,
    /// The game is focused again.
    Regained,
}

/// Tracks whether the target window is focused.
#[derive(Debug, Default)]
pub struct FocusMonitor {
    target: Option<String>,
    paused: bool,
}

impl FocusMonitor {
    /// The window to watch (`None` stops watching and clears a pause).
    pub fn set_target(&mut self, target: Option<&str>) {
        let t = target
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty());
        if t != self.target {
            self.paused = false;
        }
        self.target = t;
    }

    /// Currently paused by this monitor.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Whether `snapshot` has the target focused; `None` while unset.
    pub fn target_focused(&self, snapshot: &[Toplevel]) -> Option<bool> {
        let target = self.target.as_deref()?;
        Some(
            snapshot
                .iter()
                .any(|t| t.activated && t.title.to_lowercase().contains(target)),
        )
    }

    /// A new snapshot; returns the transition, if any.
    pub fn observe(&mut self, snapshot: &[Toplevel]) -> Option<FocusChange> {
        let focused = self.target_focused(snapshot)?;
        match (self.paused, focused) {
            (false, false) => {
                self.paused = true;
                Some(FocusChange::Lost)
            }
            (true, true) => {
                self.paused = false;
                Some(FocusChange::Regained)
            }
            _ => None,
        }
    }

    /// Clears a pause without reporting (the target changed under us).
    pub fn reset(&mut self) {
        self.paused = false;
    }
}
