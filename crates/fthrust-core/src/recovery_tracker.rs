//! Monotonic recovery evidence for keyed, recoverable UI incidents.
//!
//! Ports `reference/FTHR_UI/core/error_recovery.py` and the incident-advance
//! loop from `main.py`. A single frame or a wall-clock delay cannot clear an
//! incident: the same generation must show repeated progress for a sustained
//! interval.

use std::collections::{BTreeMap, HashMap};

use crate::capture_health::HealthState;

#[derive(Clone, Debug)]
struct ProgressWindow {
    process_generation: u64,
    backend_generation: u64,
    last_frame_count: u64,
    started_at: Option<f64>,
    progressing_polls: u32,
}

/// Requires stable, repeated frame progress before clearing an incident.
#[derive(Clone, Debug, Default)]
pub struct RecoveryTracker {
    windows: HashMap<String, ProgressWindow>,
}

impl RecoveryTracker {
    /// Progressing polls required before an incident clears.
    pub const MIN_PROGRESS_POLLS: u32 = 3;
    /// Seconds the progressing polls must span.
    pub const STABLE_FOR_SECONDS: f64 = 3.0;

    /// An empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resets one incident's window, or every window when `key` is `None`.
    pub fn reset(&mut self, key: Option<&str>) {
        match key {
            None => self.windows.clear(),
            Some(k) => {
                self.windows.remove(k);
            }
        }
    }

    /// Returns true only after eligible stable progress for this incident.
    ///
    /// Disconnects, non-eligible health states, generation changes, counter
    /// decreases and non-progressing polls all reset the window. Content
    /// suspicion is eligible only when `allow_content_suspect` is set.
    #[allow(clippy::too_many_arguments)]
    pub fn observe(
        &mut self,
        key: &str,
        connected: bool,
        process_generation: u64,
        backend_generation: u64,
        frame_count: u64,
        health_state: HealthState,
        now: f64,
        allow_content_suspect: bool,
    ) -> bool {
        let eligible = health_state == HealthState::Healthy
            || (allow_content_suspect && health_state == HealthState::ContentSuspect);
        if !connected || !eligible {
            self.windows.remove(key);
            return false;
        }
        let needs_new = self.windows.get(key).is_none_or(|w| {
            w.process_generation != process_generation
                || w.backend_generation != backend_generation
                || frame_count < w.last_frame_count
        });
        if needs_new {
            self.windows.insert(
                key.to_string(),
                ProgressWindow {
                    process_generation,
                    backend_generation,
                    last_frame_count: frame_count,
                    started_at: None,
                    progressing_polls: 0,
                },
            );
            return false;
        }
        let window = self.windows.get_mut(key).expect("present");
        if frame_count <= window.last_frame_count {
            window.started_at = None;
            window.progressing_polls = 0;
            return false;
        }
        window.last_frame_count = frame_count;
        let Some(started) = window.started_at else {
            window.started_at = Some(now);
            window.progressing_polls = 1;
            return false;
        };
        window.progressing_polls += 1;
        if window.progressing_polls >= Self::MIN_PROGRESS_POLLS
            && now - started >= Self::STABLE_FOR_SECONDS
        {
            self.windows.remove(key);
            return true;
        }
        false
    }
}

/// How a keyed incident may recover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryKind {
    /// Needs a newer engine process generation plus stable progress.
    EngineLineage,
    /// Needs a newer process generation, hardware encoding and healthy progress.
    EncoderInit,
    /// Needs stable progress; content suspicion counts as progress.
    CaptureBackend,
}

impl RecoveryKind {
    /// Whether `content_suspect` polls count as eligible progress.
    pub fn allows_content_suspect(self) -> bool {
        self == Self::CaptureBackend
    }

    /// The reference's incident `recovery_kind` string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EngineLineage => "engine_lineage",
            Self::EncoderInit => "encoder_init",
            Self::CaptureBackend => "capture_backend",
        }
    }
}

/// A keyed error-bar incident that can clear itself on stable progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoverableIncident {
    /// How it may recover.
    pub kind: RecoveryKind,
    /// Engine process generation the incident was raised in.
    pub process_generation: u64,
    /// Public `Error NNN` code shown to the user.
    pub public_error_code: String,
    /// Error-bar title.
    pub title: String,
}

/// Advances every tracked incident from one status poll and removes the
/// recovered ones, returning them in key order.
///
/// Without an active process generation nothing can recover and encoder
/// windows are reset. Encoder incidents additionally need a newer process
/// generation, a connected engine, hardware encoding and a healthy state;
/// lineage incidents need a newer process generation.
#[allow(clippy::too_many_arguments)]
pub fn advance_recoverable_incidents(
    tracker: &mut RecoveryTracker,
    incidents: &mut BTreeMap<String, RecoverableIncident>,
    active_process_generation: Option<u64>,
    connected: bool,
    frame_count: u64,
    health_state: HealthState,
    backend_generation: u64,
    hardware_active: bool,
    now: f64,
) -> Vec<(String, RecoverableIncident)> {
    let Some(active) = active_process_generation else {
        for (key, incident) in incidents.iter() {
            if incident.kind == RecoveryKind::EncoderInit {
                tracker.reset(Some(key));
            }
        }
        return Vec::new();
    };
    let mut recovered = Vec::new();
    for (key, incident) in incidents.iter() {
        match incident.kind {
            RecoveryKind::EncoderInit
                if active <= incident.process_generation
                    || !connected
                    || !hardware_active
                    || health_state != HealthState::Healthy =>
            {
                tracker.reset(Some(key));
                continue;
            }
            RecoveryKind::EngineLineage if active <= incident.process_generation => {
                tracker.reset(Some(key));
                continue;
            }
            _ => {}
        }
        if tracker.observe(
            key,
            connected,
            active,
            backend_generation,
            frame_count,
            health_state,
            now,
            incident.kind.allows_content_suspect(),
        ) {
            recovered.push(key.clone());
        }
    }
    recovered
        .into_iter()
        .filter_map(|key| incidents.remove(&key).map(|incident| (key, incident)))
        .collect()
}
