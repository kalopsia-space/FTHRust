//! Global hotkey bindings stored in `~/.fthr/hotkeys.json`.
//!
//! The Python predecessor wrote this file in several shapes (flat map,
//! nested `{keyboard, controller}` per action, and a legacy top-level
//! `keyboard` map). [`HotkeyBindings::load`] reads all of them.

pub mod hyprland;

use crate::error::{CoreError, Result};
use crate::{FthrPaths, read_json, write_json_atomic};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const CUSTOM_PREFIX: &str = "save_custom_clip_";
const MIN_CUSTOM_SECONDS: u32 = 5;
const MAX_CUSTOM_SECONDS: u32 = 900;
const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Win"];
const FIXED_IDS: [&str; 6] = [
    "save_clip",
    "save_screenshot",
    "start_recording",
    "stop_recording",
    "confirm_game_detection",
    "dismiss_game_detection",
];
const CONTROLLER_ORDER: [&str; 17] = [
    "LT",
    "RT",
    "LB",
    "RB",
    "Back",
    "Start",
    "Share",
    "LS",
    "RS",
    "DPad Up",
    "DPad Down",
    "DPad Left",
    "DPad Right",
    "Y",
    "B",
    "A",
    "X",
];

/// A bindable global hotkey action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HotkeyAction {
    /// Save the replay buffer as a clip.
    SaveClip,
    /// Take a screenshot.
    SaveScreenshot,
    /// Start a recording.
    StartRecording,
    /// Stop the current recording.
    StopRecording,
    /// Confirm a detected game.
    ConfirmGameDetection,
    /// Dismiss game detection.
    DismissGameDetection,
    /// Save a clip of the given length in seconds (5..=900).
    SaveCustomClip(u32),
}

impl HotkeyAction {
    /// Stable identifier used as the key in `hotkeys.json`.
    pub fn id(&self) -> String {
        match self {
            Self::SaveClip => "save_clip".into(),
            Self::SaveScreenshot => "save_screenshot".into(),
            Self::StartRecording => "start_recording".into(),
            Self::StopRecording => "stop_recording".into(),
            Self::ConfirmGameDetection => "confirm_game_detection".into(),
            Self::DismissGameDetection => "dismiss_game_detection".into(),
            Self::SaveCustomClip(n) => format!("{CUSTOM_PREFIX}{n}"),
        }
    }

    /// Parses an action id, accepting the legacy `game_capture_*` names.
    ///
    /// Custom clip lengths outside 5..=900 yield `None`.
    pub fn parse(id: &str) -> Option<Self> {
        Some(match id {
            "save_clip" => Self::SaveClip,
            "save_screenshot" => Self::SaveScreenshot,
            "start_recording" => Self::StartRecording,
            "stop_recording" => Self::StopRecording,
            "confirm_game_detection" | "game_capture_accept" => Self::ConfirmGameDetection,
            "dismiss_game_detection" | "game_capture_dismiss" => Self::DismissGameDetection,
            _ => Self::SaveCustomClip(custom_clip_seconds(id)?),
        })
    }

    /// User-facing description shown by the desktop shortcut dialog.
    pub fn description(&self) -> String {
        match self {
            Self::SaveClip => "Save clip".into(),
            Self::SaveScreenshot => "Save screenshot".into(),
            Self::StartRecording => "Start recording".into(),
            Self::StopRecording => "Stop recording".into(),
            Self::ConfirmGameDetection => "Confirm detected game".into(),
            Self::DismissGameDetection => "Dismiss game detection".into(),
            Self::SaveCustomClip(n) => format!("Save {n}-second clip"),
        }
    }
}

fn custom_clip_seconds(id: &str) -> Option<u32> {
    let digits = id.strip_prefix(CUSTOM_PREFIX)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    (MIN_CUSTOM_SECONDS..=MAX_CUSTOM_SECONDS)
        .contains(&n)
        .then_some(n)
}

/// Python `str.title()` for a single token.
fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_alpha = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_alpha {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_alpha = true;
        } else {
            out.push(c);
            prev_alpha = false;
        }
    }
    out
}

fn modifier_alias(key: &str) -> Option<&'static str> {
    Some(match key {
        "ctrl" | "control" => "Ctrl",
        "alt" => "Alt",
        "shift" => "Shift",
        "win" | "windows" | "meta" | "cmd" | "command" => "Win",
        _ => return None,
    })
}

/// Canonicalises a keyboard chord for display, storage and registration.
///
/// Modifiers come first in `Ctrl, Alt, Shift, Win` order, single letters are
/// upper-cased, `F1..F24` are normalised and other keys are Title-cased.
/// Always returns `Some`; an empty input gives an empty string.
pub fn normalize_keyboard_combo(input: &str) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut mods: Vec<String> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for raw in input.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        let lower = raw.to_lowercase().replace('_', " ");
        let mut chars = raw.chars();
        let single = match (chars.next(), chars.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        };
        let canonical = if let Some(m) = modifier_alias(&lower) {
            m.to_string()
        } else if let Some(c) = single.filter(|c| c.is_alphabetic()) {
            c.to_uppercase().collect()
        } else if let Some(c) = single.filter(|c| c.is_ascii_digit()) {
            c.to_string()
        } else if let Some(n) = lower
            .strip_prefix('f')
            .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|r| r.parse::<u32>().ok())
            .filter(|n| (1..=24).contains(n))
        {
            format!("F{n}")
        } else {
            title_case(raw)
        };
        if seen.contains(&canonical) {
            continue;
        }
        seen.push(canonical.clone());
        if MODIFIERS.contains(&canonical.as_str()) {
            mods.push(canonical);
        } else {
            keys.push(canonical);
        }
    }
    let mut parts: Vec<String> = MODIFIERS
        .iter()
        .filter(|m| mods.iter().any(|x| x == **m))
        .map(|m| m.to_string())
        .collect();
    parts.extend(keys);
    Some(parts.join("+"))
}

fn controller_alias(key: &str) -> Option<&'static str> {
    Some(match key {
        "a" => "A",
        "b" => "B",
        "x" => "X",
        "y" => "Y",
        "lb" | "left bumper" | "l1" => "LB",
        "rb" | "right bumper" | "r1" => "RB",
        "lt" | "left trigger" | "l2" => "LT",
        "rt" | "right trigger" | "r2" => "RT",
        "back" | "select" | "view" => "Back",
        "start" | "menu" => "Start",
        "share" | "capture" | "screenshot" | "misc" | "misc 1" => "Share",
        "ls" | "left stick" | "l3" => "LS",
        "rs" | "right stick" | "r3" => "RS",
        "dpad up" | "d-pad up" | "up" => "DPad Up",
        "dpad down" | "d-pad down" | "down" => "DPad Down",
        "dpad left" | "d-pad left" | "left" => "DPad Left",
        "dpad right" | "d-pad right" | "right" => "DPad Right",
        _ => return None,
    })
}

/// `HID:<device>:B<n>.<m>` raw button tokens (case-insensitive).
fn is_raw_controller_token(v: &str) -> bool {
    let v = v.trim();
    let parts: Vec<&str> = v.split(':').collect();
    let [h, dev, btn] = parts[..] else {
        return false;
    };
    let Some(rest) = btn.strip_prefix(['B', 'b']) else {
        return false;
    };
    let Some((a, b)) = rest.split_once('.') else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    h.eq_ignore_ascii_case("hid") && !dev.is_empty() && digits(a) && digits(b)
}

fn normalize_controller_combo(input: &str) -> String {
    let mut known: Vec<&str> = Vec::new();
    let mut raw: Vec<String> = Vec::new();
    for part in input.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        let key = part.to_lowercase().replace('_', " ");
        let canonical = controller_alias(&key).unwrap_or(part);
        if let Some(b) = CONTROLLER_ORDER.iter().find(|b| **b == canonical) {
            if !known.contains(b) {
                known.push(b);
            }
        } else if is_raw_controller_token(canonical) && !raw.iter().any(|r| r == canonical) {
            raw.push(canonical.to_string());
        }
    }
    let mut out: Vec<String> = CONTROLLER_ORDER
        .iter()
        .filter(|b| known.contains(b))
        .map(|b| b.to_string())
        .collect();
    out.extend(raw);
    out.join("+")
}

/// The xkb keysym name of a named key (`Page Up` → `Prior`), shared by the
/// portal trigger and the Hyprland bind writers.
pub(crate) fn portal_key(part: &str) -> Option<&'static str> {
    Some(match part {
        "Esc" => "Escape",
        "Enter" => "Return",
        "Space" => "space",
        "Tab" => "Tab",
        "Backspace" => "BackSpace",
        "Delete" => "Delete",
        "Insert" => "Insert",
        "Home" => "Home",
        "End" => "End",
        "Page Up" => "Prior",
        "Page Down" => "Next",
        "Up" => "Up",
        "Down" => "Down",
        "Left" => "Left",
        "Right" => "Right",
        "Print" => "Print",
        "Pause" => "Pause",
        "Scroll Lock" => "Scroll_Lock",
        _ => return None,
    })
}

/// Converts an FTHR combo such as `Ctrl+Shift+S` to the freedesktop
/// shortcuts-spec trigger `CTRL+SHIFT+s` (`Win` becomes `LOGO`).
pub fn to_portal_trigger(combo: &str) -> String {
    combo
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|part| match part {
            "Ctrl" => "CTRL".to_string(),
            "Alt" => "ALT".to_string(),
            "Shift" => "SHIFT".to_string(),
            "Win" => "LOGO".to_string(),
            _ => {
                if let Some(k) = portal_key(part) {
                    k.to_string()
                } else if part.chars().count() == 1 && part.chars().all(char::is_alphabetic) {
                    part.to_lowercase()
                } else {
                    part.to_string()
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Keyboard and controller bindings for every global action.
///
/// Maps are keyed by action id; an empty string means unbound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBindings {
    map: BTreeMap<String, String>,
    controller: BTreeMap<String, String>,
}

impl HotkeyBindings {
    /// Factory defaults: clip F9, screenshot F12, game detection F8/F7.
    pub fn defaults() -> Self {
        let map = [
            ("save_clip", "F9"),
            ("save_screenshot", "F12"),
            ("start_recording", ""),
            ("stop_recording", ""),
            ("confirm_game_detection", "F8"),
            ("dismiss_game_detection", "F7"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let controller = FIXED_IDS
            .iter()
            .map(|k| (k.to_string(), String::new()))
            .collect();
        Self { map, controller }
    }

    /// Keyboard combo bound to `id`, if the action exists.
    pub fn get(&self, id: &str) -> Option<&str> {
        self.map.get(id).map(String::as_str)
    }

    /// Controller combo bound to `id`, if the action exists.
    pub fn controller(&self, id: &str) -> Option<&str> {
        self.controller.get(id).map(String::as_str)
    }

    /// Sets (or clears, with an empty combo) the keyboard binding for `id`.
    /// The combo is normalised; an unknown id is added.
    pub fn set_keyboard(&mut self, id: &str, combo: &str) {
        let value = normalize_keyboard_combo(combo).unwrap_or_default();
        self.map.insert(id.to_string(), value);
    }

    /// Clears the controller binding for `id`.
    pub fn clear_controller(&mut self, id: &str) {
        if let Some(v) = self.controller.get_mut(id) {
            v.clear();
        }
    }

    /// The whole keyboard map (action id to combo).
    pub fn keyboard_map(&self) -> &BTreeMap<String, String> {
        &self.map
    }

    /// The whole controller map (action id to combo).
    pub fn controller_map(&self) -> &BTreeMap<String, String> {
        &self.controller
    }

    /// Loads `hotkeys.json`, returning the bindings and whether the file
    /// used a legacy shape and should be rewritten.
    ///
    /// A missing, unparsable or non-object file yields the defaults, like
    /// the Python loader.
    pub fn load(paths: &FthrPaths) -> Result<(Self, bool)> {
        let mut b = Self::defaults();
        let saved = match read_json(&paths.hotkeys_file()) {
            Ok(Some(Value::Object(o))) => o,
            Ok(_) | Err(CoreError::Json { .. }) => return Ok((b, false)),
            Err(e) => {
                tracing::warn!("could not read hotkeys.json: {e}; using defaults");
                return Ok((b, false));
            }
        };
        let keyboard_map = saved.get("keyboard").and_then(Value::as_object);
        let controller_map = saved.get("controller").and_then(Value::as_object);
        let mappings: [Option<&Map<String, Value>>; 3] =
            [Some(&saved), keyboard_map, controller_map];

        for m in mappings.iter().flatten() {
            for k in m.keys() {
                if custom_clip_seconds(k).is_some() {
                    b.map.entry(k.clone()).or_default();
                    b.controller.entry(k.clone()).or_default();
                }
            }
        }
        let mut migrated = mappings
            .iter()
            .flatten()
            .any(|m| m.contains_key("save_extended_clip"));

        let actions: Vec<String> = b.map.keys().cloned().collect();
        for action in actions {
            let mut source = action.as_str();
            if !saved.contains_key(source) {
                let legacy = match source {
                    "confirm_game_detection" => "game_capture_accept",
                    "dismiss_game_detection" => "game_capture_dismiss",
                    other => other,
                };
                if legacy != source && saved.contains_key(legacy) {
                    migrated = true;
                }
                source = legacy;
            }
            let mut value = saved.get(source);
            if let Some(km) = keyboard_map {
                value = km.get(source).or(value);
                migrated = true;
            }
            let (kv, cv) = match value {
                Some(Value::Object(o)) => (o.get("keyboard"), o.get("controller")),
                v => (v, controller_map.and_then(|c| c.get(source))),
            };
            if let Some(Value::String(s)) = kv {
                let n = if s.trim().is_empty() {
                    String::new()
                } else {
                    normalize_keyboard_combo(s).unwrap_or_default()
                };
                b.map.insert(action.clone(), n);
            } else if value.is_some_and(|v| !v.is_null()) {
                migrated = true;
            }
            if let Some(Value::String(s)) = cv {
                b.controller
                    .insert(action.clone(), normalize_controller_combo(s));
                migrated = true;
            }
        }
        Ok((b, migrated))
    }

    /// Writes `hotkeys.json` atomically: nested `{keyboard, controller}`
    /// per action when any controller binding is set, otherwise flat.
    pub fn save(&self, paths: &FthrPaths) -> Result<()> {
        let file = paths.hotkeys_file();
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let nested = self.controller.values().any(|v| !v.is_empty());
        let mut out = Map::new();
        for (id, combo) in &self.map {
            let v = if nested {
                let ctrl = self.controller.get(id).cloned().unwrap_or_default();
                serde_json::json!({ "keyboard": combo, "controller": ctrl })
            } else {
                Value::String(combo.clone())
            };
            out.insert(id.clone(), v);
        }
        write_json_atomic(&file, &Value::Object(out), true)
    }

    /// Lengths (seconds) of the custom clip actions, ascending.
    pub fn custom_clip_lengths(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .map
            .keys()
            .filter_map(|k| custom_clip_seconds(k))
            .collect();
        v.sort_unstable();
        v
    }

    /// Makes the custom clip actions match `lengths`: missing ones are added
    /// unbound and absent ones removed. Lengths outside 5..=900 are ignored.
    pub fn sync_custom_clips(&mut self, lengths: &[u32]) {
        let wanted: BTreeSet<String> = lengths
            .iter()
            .filter(|n| (MIN_CUSTOM_SECONDS..=MAX_CUSTOM_SECONDS).contains(n))
            .map(|n| HotkeyAction::SaveCustomClip(*n).id())
            .collect();
        let stale: Vec<String> = self
            .map
            .keys()
            .filter(|k| custom_clip_seconds(k).is_some() && !wanted.contains(*k))
            .cloned()
            .collect();
        for k in stale {
            self.map.remove(&k);
            self.controller.remove(&k);
        }
        for k in wanted {
            self.map.entry(k.clone()).or_default();
            self.controller.entry(k).or_default();
        }
    }
}

/// Loads the set of actions the user cleared while the desktop portal still
/// holds a key for them. Missing or unreadable files give an empty set.
pub fn load_portal_cleared(paths: &FthrPaths) -> Result<BTreeSet<String>> {
    match read_json(&paths.hotkeys_portal_cleared_file()) {
        Ok(Some(Value::Array(items))) => Ok(items
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()),
        Ok(_) | Err(CoreError::Json { .. }) => Ok(BTreeSet::new()),
        Err(e) => {
            tracing::warn!("could not read hotkeys-portal-cleared.json: {e}");
            Ok(BTreeSet::new())
        }
    }
}

/// Saves the cleared-action set as a sorted JSON list.
pub fn save_portal_cleared(paths: &FthrPaths, cleared: &BTreeSet<String>) -> Result<()> {
    let file = paths.hotkeys_portal_cleared_file();
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let list: Vec<Value> = cleared.iter().cloned().map(Value::String).collect();
    write_json_atomic(&file, &Value::Array(list), false)
}
