//! Application state shared by the shell, pages, popups and dialogs. Pages
//! are functions of this state so the test harness renders them without an
//! engine.

use std::path::Path;

use fthrust_core::FthrPaths;
use fthrust_core::settings::SettingsStore;
use serde_json::Value;

use crate::icons::IconCache;
use crate::theme;
use crate::tokens::Tokens;
use crate::widgets::popup::PanelState;

/// Main pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Page {
    /// Clip library.
    #[default]
    Clips,
    /// Settings.
    Settings,
}

/// Settings tabs, in bar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(missing_docs)]
pub enum Tab {
    #[default]
    General,
    Clip,
    Audio,
    Visuals,
    Customize,
    Performance,
    Shortcuts,
}

impl Tab {
    /// All tabs in bar order.
    pub const ALL: [Tab; 7] = [
        Tab::General,
        Tab::Clip,
        Tab::Audio,
        Tab::Visuals,
        Tab::Customize,
        Tab::Performance,
        Tab::Shortcuts,
    ];

    /// Bar label.
    pub fn label(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Clip => "Clip",
            Tab::Audio => "Audio",
            Tab::Visuals => "Visuals",
            Tab::Customize => "Customize",
            Tab::Performance => "Performance",
            Tab::Shortcuts => "Shortcuts",
        }
    }

    /// Position in the bar.
    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// The tab `steps` to the right (wrapping).
    pub fn cycle(self, steps: i32) -> Tab {
        let n = Tab::ALL.len() as i32;
        Tab::ALL[((self.index() as i32 + steps).rem_euclid(n)) as usize]
    }
}

/// Capture status label style (`status_*_qss`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StatusStyle {
    /// ACCENT.
    Active,
    /// TEXT_MUTED.
    #[default]
    Idle,
    /// ERROR.
    Warning,
}

/// The toolbar status label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusText {
    /// Raw text (capitalised when drawn).
    pub text: String,
    /// Style.
    pub style: StatusStyle,
}

impl Default for StatusText {
    fn default() -> Self {
        StatusText {
            text: "Starting capture".into(),
            style: StatusStyle::Idle,
        }
    }
}

/// Open state of the four top-bar popups.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PopupState {
    /// Capture settings.
    pub capture: PanelState,
    /// Source.
    pub source: PanelState,
    /// Game detection.
    pub game: PanelState,
    /// Hotkeys.
    pub hotkeys: PanelState,
}

/// Dialog kinds the shell can show (filled by the dialogs module).
#[derive(Clone, Debug, PartialEq)]
pub enum DialogKind {
    /// Non-modal information / warning message with OK.
    Message {
        /// Title.
        title: String,
        /// Body.
        message: String,
    },
    /// Modal yes/no question.
    Question {
        /// Title.
        title: String,
        /// Body.
        message: String,
        /// Identifies the asker.
        tag: String,
    },
    /// Modal text input.
    Input {
        /// Title.
        title: String,
        /// Prompt label.
        label: String,
        /// Current text.
        text: String,
        /// Identifies the asker.
        tag: String,
    },
    /// Custom clip length picker.
    CustomClipLength {
        /// Seconds.
        seconds: u32,
    },
    /// Export preset editor.
    ExportPreset {
        /// Preset being edited (`None` = new).
        preset: Option<fthrust_core::export_presets::ExportPreset>,
    },
    /// Diagnostic report result.
    DiagnosticReport {
        /// ZIP path.
        zip: std::path::PathBuf,
    },
}

/// A dialog outcome handed back to the shell.
#[derive(Clone, Debug, PartialEq)]
pub enum DialogResult {
    /// Question answered.
    Answer {
        /// Asker tag.
        tag: String,
        /// Yes.
        yes: bool,
    },
    /// Input accepted (`None` = cancelled).
    Input {
        /// Asker tag.
        tag: String,
        /// Text.
        text: Option<String>,
    },
    /// Custom clip length chosen.
    ClipLength(Option<u32>),
    /// Export preset saved.
    ExportPreset(Option<fthrust_core::export_presets::ExportPreset>),
}

/// Pending dialogs: one modal at a time, messages stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DialogQueue {
    /// Modal dialogs, front first.
    pub modal: std::collections::VecDeque<DialogKind>,
    /// Non-modal messages, oldest first.
    pub messages: Vec<DialogKind>,
    /// Results produced this frame.
    pub results: Vec<DialogResult>,
}

impl DialogQueue {
    /// Queues a modal dialog.
    pub fn push_modal(&mut self, kind: DialogKind) {
        self.modal.push_back(kind);
    }

    /// Shows a non-modal message.
    pub fn push_message(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.messages.push(DialogKind::Message {
            title: title.into(),
            message: message.into(),
        });
    }

    /// Takes the results produced so far.
    pub fn take_results(&mut self) -> Vec<DialogResult> {
        std::mem::take(&mut self.results)
    }
}

/// An open clip editor (plan 6 fills these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    /// Session number (1-based).
    pub number: usize,
    /// Clip file stem.
    pub stem: String,
}

/// A connected monitor (plan 5 fills these from the engine).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Output name.
    pub name: String,
    /// Logical width.
    pub width: u32,
    /// Logical height.
    pub height: u32,
    /// Refresh rate in Hz.
    pub refresh_hz: u32,
}

/// Window flags the shell reads each frame.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowState {
    /// Maximised.
    pub maximized: bool,
    /// Fullscreen.
    pub fullscreen: bool,
    /// Close requested (title-strip close or Ctrl+Q).
    pub close_requested: bool,
    /// Last known inner size (persisted on exit).
    pub last_inner: fthrust_core::window_geometry::Rect,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            maximized: false,
            fullscreen: false,
            close_requested: false,
            last_inner: fthrust_core::window_geometry::Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
        }
    }
}

/// Everything the UI draws from.
pub struct AppState {
    /// Profile paths.
    pub paths: FthrPaths,
    /// Settings store.
    pub settings: SettingsStore,
    /// Theme tokens.
    pub tokens: Tokens,
    /// Icon textures.
    pub icons: IconCache,
    /// Toolbar status.
    pub status: StatusText,
    /// Current page.
    pub page: Page,
    /// Current settings tab.
    pub settings_tab: Tab,
    /// Popups.
    pub popups: PopupState,
    /// Dialogs.
    pub dialogs: DialogQueue,
    /// Editor sessions.
    pub editor_sessions: Vec<SessionSummary>,
    /// Monitors.
    pub monitors: Vec<MonitorInfo>,
    /// Window flags.
    pub window: WindowState,
    /// Capture source summary for the toolbar (plan 5 updates it).
    pub source_label: String,
    /// Settings keys changed since the last engine configure (plan 5 consumes).
    pub changed_keys: Vec<&'static str>,
    /// Bottom error bar.
    pub error_bar: crate::shell::error_bar::ErrorBarState,
    /// Running page slide.
    pub page_slide: crate::shell::page_stack::PageSlide,
    /// Toolbar button rects (popup anchors): capture, source, game, hotkeys.
    pub toolbar_anchors: [egui::Rect; 4],
    /// Manual recording control enabled (plan 5).
    pub record_enabled: bool,
    /// Record clicked this frame (plan 5 consumes).
    pub record_requested: bool,
    /// Rail editor button clicked (plan 6 consumes).
    pub editor_requested: Option<usize>,
    /// Rail editor close requested (plan 6 consumes).
    pub editor_close_requested: Option<usize>,
    /// Keyboard / controller bindings (`hotkeys.json`).
    pub hotkeys: fthrust_core::hotkeys::HotkeyBindings,
    /// Bindings changed since the hotkey service last read them (plan 5).
    pub hotkeys_changed: bool,
    /// The desktop portal owns the shortcuts (plan 5 sets it).
    pub hotkeys_portal_active: bool,
    /// Action id currently recording a chord.
    pub hotkey_recording: Option<String>,
    /// Hotkey socket path shown in the setup instructions.
    pub hotkey_socket_path: String,
    /// `nc` executable shown in the setup instructions.
    pub nc_path: String,
    /// Detected compositor (`hyprland`, `kwin`, `gnome`, `unknown`, `none`).
    pub compositor: String,
    /// Running windows for game detection (plan 5 fills, `display_name`s).
    pub running_windows: Vec<String>,
    /// Selected running window index.
    pub running_window_index: usize,
    /// Capture settings profile shown in the popup.
    pub capture_profile: CaptureProfile,
    /// A capture setting changed that needs an engine restart (Apply shows).
    pub restart_pending: bool,
    /// Apply clicked: restart the engine (plan 5 consumes).
    pub engine_restart_requested: bool,
    /// Refresh the running-window list (plan 5 consumes).
    pub refresh_windows_requested: bool,
    /// Open the desktop's shortcut settings (plan 5 consumes).
    pub open_desktop_shortcuts_requested: bool,
}

/// Capture settings profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CaptureProfile {
    /// Replay clips.
    #[default]
    Clips,
    /// Manual recordings.
    Recording,
}

impl AppState {
    /// Loads settings and theme from `paths`.
    pub fn load(paths: FthrPaths) -> fthrust_core::Result<AppState> {
        let settings = SettingsStore::load(paths.clone())?;
        let mut tokens = theme::load(&paths);
        tokens.reduce_motion = settings.get_bool("reduce_ui_animations").unwrap_or(false);
        let error_bar = crate::shell::error_bar::ErrorBarState::new(
            settings
                .get_bool("error_notifications_enabled")
                .unwrap_or(true),
        );
        let (hotkeys, _) = fthrust_core::hotkeys::HotkeyBindings::load(&paths)
            .unwrap_or_else(|_| (fthrust_core::hotkeys::HotkeyBindings::defaults(), false));
        let hotkey_socket_path = fthrust_core::runtime_dir()
            .map(|d| d.join("hotkey.sock").display().to_string())
            .unwrap_or_else(|_| "<socket unavailable>".into());
        let nc_path = ["/usr/bin/nc", "/bin/nc", "/usr/local/bin/nc"]
            .iter()
            .find(|p| Path::new(p).exists())
            .map(|p| (*p).to_owned())
            .unwrap_or_else(|| "nc".into());
        Ok(AppState {
            paths,
            settings,
            tokens,
            icons: IconCache::default(),
            status: StatusText::default(),
            page: Page::Clips,
            settings_tab: Tab::General,
            popups: PopupState::default(),
            dialogs: DialogQueue::default(),
            editor_sessions: Vec::new(),
            monitors: Vec::new(),
            window: WindowState::default(),
            source_label: "Source".into(),
            changed_keys: Vec::new(),
            error_bar,
            page_slide: Default::default(),
            toolbar_anchors: [egui::Rect::NOTHING; 4],
            record_enabled: false,
            record_requested: false,
            editor_requested: None,
            editor_close_requested: None,
            hotkeys,
            hotkeys_changed: false,
            hotkeys_portal_active: false,
            hotkey_recording: None,
            hotkey_socket_path,
            nc_path,
            compositor: crate::popups::hotkeys::detect_compositor(),
            running_windows: Vec::new(),
            running_window_index: 0,
            capture_profile: CaptureProfile::Clips,
            restart_pending: false,
            engine_restart_requested: false,
            refresh_windows_requested: false,
            open_desktop_shortcuts_requested: false,
        })
    }

    /// A state over a fresh profile at `home` seeded with `settings`.
    pub fn for_tests(home: &Path, settings: Value) -> AppState {
        let paths = FthrPaths::new(home);
        std::fs::create_dir_all(paths.root()).expect("profile dir");
        if let Value::Object(map) = &settings
            && !map.is_empty()
        {
            std::fs::write(
                paths.settings_file(),
                serde_json::to_string_pretty(&settings).unwrap(),
            )
            .expect("settings");
        }
        let mut state = AppState::load(paths).expect("state");
        state.tokens.wayland = true;
        state
    }

    /// Reads a bool setting with the reference default.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        self.settings.get_bool(key).unwrap_or(default)
    }

    /// Sets a setting and records the key for the next engine configure.
    pub fn set_setting(&mut self, key: &'static str, value: Value) {
        self.settings.set(key, value);
        if !self.changed_keys.contains(&key) {
            self.changed_keys.push(key);
        }
        if let Err(e) = self.settings.save() {
            tracing::warn!("settings not saved: {e}");
        }
    }
}
