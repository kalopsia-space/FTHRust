//! In-app keyboard navigation bindings (never global OS hotkeys).
//!
//! Bindings live in `settings.json` under `navigation_bindings`,
//! `shortcut_categories_enabled` and `shortcuts_enabled`.

use crate::settings::SettingsStore;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Where a binding is active; used by the conflict rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Active everywhere in the application.
    Application,
    /// Active on the main window.
    Main,
    /// Active in the clip library.
    Library,
    /// Active in the settings view.
    Settings,
    /// Active in the clip editor.
    Editor,
}

/// Static description of one bindable action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingDefinition {
    /// Stable action identifier (settings key).
    pub action: &'static str,
    /// Human-readable label.
    pub label: &'static str,
    /// Default key combo.
    pub default: &'static str,
    /// Scope in which the binding is active.
    pub scope: Scope,
    /// Settings category used for enabling or disabling groups.
    pub category: &'static str,
}

/// All bindable actions, in display order.
pub const BINDING_DEFINITIONS: [BindingDefinition; 22] = [
    BindingDefinition {
        action: "switch_panel",
        label: "Switch Clips / Settings",
        default: "Tab+S",
        scope: Scope::Application,
        category: "Application",
    },
    BindingDefinition {
        action: "sidebar_next",
        label: "Next sidebar destination",
        default: "Shift+Down",
        scope: Scope::Application,
        category: "Application",
    },
    BindingDefinition {
        action: "sidebar_previous",
        label: "Previous sidebar destination",
        default: "Shift+Up",
        scope: Scope::Application,
        category: "Application",
    },
    BindingDefinition {
        action: "toggle_detection",
        label: "Toggle game detection",
        default: "Shift+G",
        scope: Scope::Main,
        category: "Application",
    },
    BindingDefinition {
        action: "restart_capture",
        label: "Restart capture engine",
        default: "Alt+Shift+R",
        scope: Scope::Main,
        category: "Application",
    },
    BindingDefinition {
        action: "library_scroll_up",
        label: "Scroll clips up",
        default: "Up",
        scope: Scope::Library,
        category: "Clips",
    },
    BindingDefinition {
        action: "library_scroll_down",
        label: "Scroll clips down",
        default: "Down",
        scope: Scope::Library,
        category: "Clips",
    },
    BindingDefinition {
        action: "category_next",
        label: "Next settings category",
        default: "Shift+Right",
        scope: Scope::Settings,
        category: "Settings",
    },
    BindingDefinition {
        action: "category_previous",
        label: "Previous settings category",
        default: "Shift+Left",
        scope: Scope::Settings,
        category: "Settings",
    },
    BindingDefinition {
        action: "editor_split",
        label: "Split segment here",
        default: "Ctrl+C",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_delete",
        label: "Delete segment",
        default: "Ctrl+D",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_restore",
        label: "Reinstate segment",
        default: "Ctrl+W",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_crop",
        label: "Quick crop",
        default: "Ctrl+Q",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_export",
        label: "Export now",
        default: "Ctrl+E",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_minimize",
        label: "Minimize editor",
        default: "Shift+Escape",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_close",
        label: "Close editor",
        default: "Escape",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_next",
        label: "Open next clip",
        default: "Tab+Right",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_previous",
        label: "Open previous clip",
        default: "Tab+Left",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_play",
        label: "Play / pause",
        default: "Space",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_undo",
        label: "Undo",
        default: "Ctrl+Z",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_redo",
        label: "Redo",
        default: "Ctrl+Y",
        scope: Scope::Editor,
        category: "Clip editor",
    },
    BindingDefinition {
        action: "editor_redo_alt",
        label: "Redo (alternate)",
        default: "Ctrl+Shift+Z",
        scope: Scope::Editor,
        category: "Clip editor",
    },
];

const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Meta"];

fn alias(lower: &str) -> Option<&'static str> {
    Some(match lower {
        "control" => "Ctrl",
        "win" | "cmd" | "super" => "Meta",
        "esc" => "Escape",
        "enter" => "Return",
        _ => return None,
    })
}

fn title_case(token: &str) -> String {
    let lower = token.to_lowercase();
    let is_fkey = lower.strip_prefix('f').is_some_and(|n| {
        n.parse::<u8>().is_ok_and(|v| (1..=24).contains(&v)) && !n.starts_with('0')
    });
    if is_fkey {
        return lower.to_uppercase();
    }
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Canonicalise a key combo: modifiers in `Ctrl, Alt, Shift, Meta` order
/// followed by the remaining keys, aliases resolved and keys Title-cased.
///
/// Returns `Some("")` for an empty (cleared) binding and `None` when the
/// combo contains only modifiers.
pub fn normalize_binding(input: &str) -> Option<String> {
    let mut tokens: Vec<String> = Vec::new();
    for raw in input.split('+') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let lower = raw.to_lowercase();
        let resolved = alias(&lower)
            .map(str::to_string)
            .unwrap_or_else(|| raw.to_string());
        let token = match MODIFIERS.iter().find(|m| m.eq_ignore_ascii_case(&resolved)) {
            Some(m) => (*m).to_string(),
            None => title_case(&resolved),
        };
        if !tokens.contains(&token) {
            tokens.push(token);
        }
    }
    if !tokens.is_empty() && tokens.iter().all(|t| MODIFIERS.contains(&t.as_str())) {
        return None;
    }
    let mut out: Vec<&str> = MODIFIERS
        .iter()
        .copied()
        .filter(|m| tokens.iter().any(|t| t == m))
        .collect();
    out.extend(
        tokens
            .iter()
            .map(String::as_str)
            .filter(|t| !MODIFIERS.contains(t)),
    );
    Some(out.join("+"))
}

fn token_set(combo: &str) -> BTreeSet<&str> {
    combo.split('+').filter(|t| !t.is_empty()).collect()
}

fn scopes_overlap(a: Scope, b: Scope) -> bool {
    use Scope::{Application, Library, Main, Settings};
    a == b
        || a == Application
        || b == Application
        || matches!(
            (a, b),
            (Main, Settings) | (Settings, Main) | (Main, Library) | (Library, Main)
        )
}

fn definition(action: &str) -> Option<&'static BindingDefinition> {
    BINDING_DEFINITIONS.iter().find(|d| d.action == action)
}

/// A requested binding clashes with another action's binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingConflict {
    /// The action being rebound.
    pub action: String,
    /// The action that already uses the combo.
    pub other_action: String,
    /// The normalised combo that clashed.
    pub combo: String,
}

/// Why [`NavigationBindings::set`] refused a binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingError {
    /// Unknown action, or a combo that cannot be normalised (modifiers only).
    Invalid(String),
    /// The combo is already used by an overlapping action.
    Conflict(BindingConflict),
}

impl std::fmt::Display for BindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(msg) => write!(f, "invalid binding: {msg}"),
            Self::Conflict(c) => write!(f, "{} already used by {}", c.combo, c.other_action),
        }
    }
}

impl std::error::Error for BindingError {}

/// The current set of navigation bindings and enable flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationBindings {
    bindings: BTreeMap<String, String>,
    categories_enabled: BTreeMap<String, bool>,
    enabled: bool,
}

impl NavigationBindings {
    /// Read bindings from settings; missing or malformed entries fall back
    /// to the definition defaults.
    pub fn from_settings(s: &SettingsStore) -> Self {
        let saved = s.get("navigation_bindings").and_then(Value::as_object);
        let bindings = BINDING_DEFINITIONS
            .iter()
            .map(|d| {
                let combo = saved
                    .and_then(|m| m.get(d.action))
                    .and_then(Value::as_str)
                    .and_then(normalize_binding)
                    .unwrap_or_else(|| d.default.to_string());
                (d.action.to_string(), combo)
            })
            .collect();
        let categories_enabled = s
            .get("shortcut_categories_enabled")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b)))
                    .collect()
            })
            .unwrap_or_default();
        let enabled = s.get_bool("shortcuts_enabled").unwrap_or(true);
        Self {
            bindings,
            categories_enabled,
            enabled,
        }
    }

    /// Write all three settings keys (does not save to disk).
    pub fn write_to(&self, s: &mut SettingsStore) {
        let to_map = |it: &mut dyn Iterator<Item = (String, Value)>| -> Value {
            Value::Object(it.collect::<Map<String, Value>>())
        };
        s.set(
            "navigation_bindings",
            to_map(
                &mut self
                    .bindings
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::from(v.clone()))),
            ),
        );
        s.set(
            "shortcut_categories_enabled",
            to_map(
                &mut self
                    .categories_enabled
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::from(*v))),
            ),
        );
        s.set("shortcuts_enabled", Value::from(self.enabled));
    }

    /// The combo bound to `action`, if the action exists.
    pub fn get(&self, action: &str) -> Option<&str> {
        self.bindings.get(action).map(String::as_str)
    }

    /// Rebind `action` to `combo` after normalising it.
    ///
    /// Fails with [`BindingError::Invalid`] for an unknown action or a
    /// modifiers-only combo, and [`BindingError::Conflict`] when another
    /// action with the same token set has an overlapping scope (same scope,
    /// either is `Application`, or the pair is Main/Settings or Main/Library).
    /// An empty combo clears the binding and never conflicts.
    pub fn set(&mut self, action: &str, combo: &str) -> Result<(), BindingError> {
        let def = definition(action)
            .ok_or_else(|| BindingError::Invalid(format!("unknown action {action}")))?;
        let norm = normalize_binding(combo)
            .ok_or_else(|| BindingError::Invalid(format!("{combo:?} needs a key")))?;
        if !norm.is_empty() {
            let wanted = token_set(&norm);
            for other in &BINDING_DEFINITIONS {
                if other.action == action || !scopes_overlap(def.scope, other.scope) {
                    continue;
                }
                let used = self.bindings.get(other.action).map_or("", String::as_str);
                if token_set(used) == wanted {
                    return Err(BindingError::Conflict(BindingConflict {
                        action: action.to_string(),
                        other_action: other.action.to_string(),
                        combo: norm,
                    }));
                }
            }
        }
        self.bindings.insert(action.to_string(), norm);
        Ok(())
    }

    /// Whether a settings category is enabled; categories never recorded
    /// count as enabled.
    pub fn is_category_enabled(&self, category: &str) -> bool {
        self.categories_enabled
            .get(category)
            .copied()
            .unwrap_or(true)
    }

    /// Enable or disable one settings category.
    pub fn set_category_enabled(&mut self, category: &str, on: bool) {
        self.categories_enabled.insert(category.to_string(), on);
    }

    /// Whether navigation shortcuts are enabled globally.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Enable or disable all navigation shortcuts.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }
}
