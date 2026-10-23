//! Game detection on Wayland (`game_detection.py` + the `main.py` handlers):
//! a background thread forwards the compositor's toplevel list; the flow on
//! the UI thread matches candidates against the custom rules and the
//! blacklist with the live settings, prompts or switches, and falls back
//! to the desktop when the game closes.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fthrust_core::game_rules::{CustomGameRule, is_blacklisted, matching_rule, normalise_rules};
use fthrust_wayland::toplevels::{Toplevel, ToplevelWatcher};
use serde_json::json;

use super::{CardKind, Event, Outbox};
use crate::state::AppState;

/// How long a prompt waits for an answer.
pub const PROMPT_TIMEOUT_S: f64 = 15.0;
/// The watcher is re-read at least this often.
pub const POLL: Duration = Duration::from_secs(2);

/// What the detector reads.
pub trait Toplevels: Send {
    /// The protocol is present.
    fn available(&self) -> bool;
    /// The current list.
    fn snapshot(&self) -> Vec<Toplevel>;
    /// Wakes when the list changed.
    fn changes(&self) -> &std::sync::mpsc::Receiver<()>;
}

impl Toplevels for ToplevelWatcher {
    fn available(&self) -> bool {
        ToplevelWatcher::available(self)
    }

    fn snapshot(&self) -> Vec<Toplevel> {
        ToplevelWatcher::snapshot(self)
    }

    fn changes(&self) -> &std::sync::mpsc::Receiver<()> {
        ToplevelWatcher::changes(self)
    }
}

/// A toplevel that looks like a game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameCandidate {
    /// The toplevel id.
    pub id: u32,
    /// Title.
    pub title: String,
    /// App id.
    pub app_id: String,
}

/// Fullscreen toplevels or custom-rule matches that are not blacklisted
/// and have a title.
pub fn candidates(
    snapshot: &[Toplevel],
    custom: &[CustomGameRule],
    blacklist: &[CustomGameRule],
) -> Vec<GameCandidate> {
    snapshot
        .iter()
        .filter(|t| !t.title.trim().is_empty())
        .filter(|t| t.fullscreen || matching_rule(custom, &t.title, &t.app_id).is_some())
        .filter(|t| !is_blacklisted(blacklist, &t.title, &t.app_id))
        .map(|t| GameCandidate {
            id: t.id,
            title: t.title.clone(),
            app_id: t.app_id.clone(),
        })
        .collect()
}

/// `"<title> · <app_id>"` for every titled toplevel (the Source popup list).
pub fn running_windows(snapshot: &[Toplevel]) -> Vec<String> {
    snapshot
        .iter()
        .filter(|t| !t.title.trim().is_empty())
        .map(|t| {
            if t.app_id.is_empty() {
                t.title.clone()
            } else {
                format!("{} · {}", t.title, t.app_id)
            }
        })
        .collect()
}

/// The background reader.
pub struct GameDetector {
    stop: Arc<AtomicBool>,
    refresh: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl GameDetector {
    /// Starts forwarding `watcher`'s snapshots as [`Event::Toplevels`].
    pub fn start(watcher: Box<dyn Toplevels>, outbox: Outbox) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            let refresh = refresh.clone();
            std::thread::Builder::new()
                .name("fthrust-game-detect".into())
                .spawn(move || run(watcher, outbox, stop, refresh))
                .ok()
        };
        GameDetector {
            stop,
            refresh,
            thread,
        }
    }

    /// Re-sends the current list.
    pub fn refresh(&self) {
        self.refresh.store(true, Ordering::SeqCst);
    }

    /// Stops the thread.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    watcher: Box<dyn Toplevels>,
    outbox: Outbox,
    stop: Arc<AtomicBool>,
    refresh: Arc<AtomicBool>,
) {
    let available = watcher.available();
    outbox.send(Event::GameDetectionAvailability(available));
    if !available {
        return;
    }
    let mut last: Option<Vec<Toplevel>> = None;
    while !stop.load(Ordering::SeqCst) {
        let snapshot = watcher.snapshot();
        let forced = refresh.swap(false, Ordering::SeqCst);
        if forced || last.as_ref() != Some(&snapshot) {
            last = Some(snapshot.clone());
            outbox.send(Event::Toplevels(snapshot));
        }
        match watcher.changes().recv_timeout(POLL) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// What the flow wants done.
#[derive(Clone, Debug, PartialEq)]
pub enum GameAction {
    /// Show a card.
    Card(CardKind),
    /// Restart the engine with the new source.
    Restart,
}

/// The UI-thread flow.
#[derive(Default)]
pub struct GameFlow {
    pending: Option<(GameCandidate, f64)>,
    active: Option<GameCandidate>,
    suppressed: BTreeSet<u32>,
    known: BTreeSet<u32>,
    last_snapshot: Vec<Toplevel>,
}

fn rules(state: &AppState, key: &str) -> Vec<CustomGameRule> {
    state
        .settings
        .get(key)
        .map(normalise_rules)
        .unwrap_or_default()
}

impl GameFlow {
    /// The active game, if capture was switched to one.
    pub fn active(&self) -> Option<&GameCandidate> {
        self.active.as_ref()
    }

    /// A new toplevel list.
    pub fn on_snapshot(
        &mut self,
        state: &mut AppState,
        snapshot: Vec<Toplevel>,
        now: f64,
    ) -> Vec<GameAction> {
        let mut out = Vec::new();
        state.running_windows = running_windows(&snapshot);
        self.last_snapshot = snapshot;
        let present: BTreeSet<u32> = self.last_snapshot.iter().map(|t| t.id).collect();
        self.suppressed.retain(|id| present.contains(id));
        self.known.retain(|id| present.contains(id));
        if let Some((p, _)) = &self.pending
            && !present.contains(&p.id)
        {
            self.pending = None;
        }
        if let Some(active) = self.active.clone()
            && !present.contains(&active.id)
        {
            self.active = None;
            out.extend(self.game_lost(state));
        }
        if !state
            .settings
            .get_bool("game_detection_enabled")
            .unwrap_or(false)
        {
            return out;
        }
        let custom = rules(state, "game_detection_custom_games");
        let blacklist = rules(state, "game_detection_blacklist");
        for c in candidates(&self.last_snapshot, &custom, &blacklist) {
            if self.known.contains(&c.id) || self.suppressed.contains(&c.id) {
                continue;
            }
            self.known.insert(c.id);
            if self.active.as_ref().is_some_and(|a| a.id == c.id) {
                continue;
            }
            let mode = state
                .settings
                .get_str("game_detection_mode")
                .unwrap_or("auto")
                .to_lowercase();
            if mode == "prompt" {
                if let Some((prev, _)) = self.pending.take() {
                    self.suppressed.insert(prev.id);
                }
                let confirm = state
                    .hotkeys
                    .get("confirm_game_detection")
                    .unwrap_or("F8")
                    .to_owned();
                let dismiss = state
                    .hotkeys
                    .get("dismiss_game_detection")
                    .unwrap_or("F7")
                    .to_owned();
                out.push(GameAction::Card(CardKind::Prompt {
                    text: format!(
                        "{} detected — [{confirm}] Record  [{dismiss}] Dismiss",
                        c.title
                    ),
                }));
                self.pending = Some((c, now + PROMPT_TIMEOUT_S));
            } else {
                out.extend(self.switch_to(state, c));
            }
        }
        out
    }

    /// The confirm hotkey.
    pub fn confirm(&mut self, state: &mut AppState) -> Vec<GameAction> {
        let Some((c, _)) = self.pending.take() else {
            return Vec::new();
        };
        let blacklist = rules(state, "game_detection_blacklist");
        if is_blacklisted(&blacklist, &c.title, &c.app_id) {
            return Vec::new();
        }
        self.switch_to(state, c)
    }

    /// The dismiss hotkey.
    pub fn dismiss(&mut self) {
        if let Some((c, _)) = self.pending.take() {
            self.suppressed.insert(c.id);
        }
    }

    /// The prompt timeout.
    pub fn on_tick(&mut self, now: f64) {
        if let Some((c, deadline)) = &self.pending
            && now >= *deadline
        {
            self.suppressed.insert(c.id);
            self.pending = None;
        }
    }

    fn switch_to(&mut self, state: &mut AppState, c: GameCandidate) -> Vec<GameAction> {
        state.set_setting("capture_mode", json!("window"));
        state.set_setting("target_window_name", json!(c.title));
        state.set_setting("target_app_id", json!(c.app_id));
        state.set_setting("target_hwnd", json!(0));
        state.source_label = c.title.to_uppercase();
        let name = c.title.clone();
        self.active = Some(c);
        vec![
            GameAction::Card(CardKind::Capturing { source: name }),
            GameAction::Restart,
        ]
    }

    fn game_lost(&mut self, state: &mut AppState) -> Vec<GameAction> {
        if !state
            .settings
            .get_bool("game_detection_fallback_desktop")
            .unwrap_or(false)
        {
            return Vec::new();
        }
        if state.settings.get_str("capture_mode") != Some("window") {
            return Vec::new();
        }
        state.set_setting("capture_mode", json!("desktop"));
        state.set_setting("target_window_name", json!(""));
        state.set_setting("target_app_id", json!(""));
        state.source_label = "DESKTOP".into();
        vec![
            GameAction::Card(CardKind::Prompt {
                text: "Game closed — switched back to Desktop".into(),
            }),
            GameAction::Restart,
        ]
    }
}

/// Shared handle so the detector can be stopped from the services.
pub type SharedDetector = Arc<Mutex<Option<GameDetector>>>;
