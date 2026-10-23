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
    /// Bumped on every modal push so drafts are reset per dialog.
    pub generation: u64,
}

impl DialogQueue {
    /// Queues a modal dialog.
    pub fn push_modal(&mut self, kind: DialogKind) {
        self.generation += 1;
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
    /// This executable, the compositor's hotkey client (`fthrust --hotkey`).
    pub client_exe: String,
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
    /// A capture-source change awaiting the Source popup's Apply.
    pub source_restart_pending: bool,
    /// An error-bar action button was pressed (its id; plan 5 consumes).
    pub error_action_requested: Option<String>,
    /// Apply clicked: restart the engine (plan 5 consumes).
    pub engine_restart_requested: bool,
    /// Refresh the running-window list (plan 5 consumes).
    pub refresh_windows_requested: bool,
    /// Open the desktop's shortcut settings (plan 5 consumes).
    pub open_desktop_shortcuts_requested: bool,
    /// Settings category slide.
    pub tab_slide: crate::settings::TabSlide,
    /// Encoder capabilities (`None` while detecting).
    pub encoder_capabilities: Option<Vec<fthrust_media::capabilities::EncoderCapability>>,
    /// Capabilities came from the saved device profile.
    pub encoder_from_cache: bool,
    /// Encoder the engine reports as active.
    pub active_encoder: Option<String>,
    /// Encoder settings changed since the last Apply.
    pub encoder_dirty: bool,
    /// Redetect encoders (plan 5 consumes).
    pub redetect_encoders_requested: bool,
    /// A folder picker was requested (plan 5 opens the portal).
    pub browse_requested: Option<BrowseKind>,
    /// Scan imported folders (plan 5 consumes).
    pub scan_clips_requested: bool,
    /// Clips directory changed (plan 5 reloads the library).
    pub clips_directory_changed: bool,
    /// Imported folders changed (plan 5 refreshes the library).
    pub imported_folders_changed: bool,
    /// Export a diagnostic report (Task 15 / plan 5 consumes).
    pub diagnostic_export_requested: bool,
    /// The last exported diagnostic ZIP (shown as a draggable file).
    pub diagnostic_report_zip: Option<std::path::PathBuf>,
    /// Selected export preset id.
    pub export_preset_selected: String,
    /// Selected settings preset name.
    pub settings_preset_selected: String,
    /// A transient status hint (plan 5 shows it on the capture card).
    pub status_hint: Option<String>,
    /// Microphone `(level, peak)` in 0..1 (plan 5 feeds it).
    pub mic_level: Option<(f32, f32)>,
    /// Which Gary handle is being dragged (0 = MIN, 1 = MAX).
    pub gary_drag: Option<u8>,
    /// Microphone devices `(name, id)` (plan 5 fills).
    pub mic_devices: Vec<(String, String)>,
    /// Playback devices `(label, id)` for capture (plan 5 fills).
    pub audio_devices: Vec<(String, String)>,
    /// Camera device labels (plan 5 fills).
    pub camera_devices: Vec<String>,
    /// Refresh audio devices (plan 5 consumes).
    pub refresh_audio_devices_requested: bool,
    /// Refresh cameras (plan 5 consumes).
    pub refresh_cameras_requested: bool,
    /// Selected image layer index.
    pub selected_image_layer: usize,
    /// Overlay box drag in progress: `(kind, resizing, start rect)`.
    pub overlay_drag: Option<(String, bool, FracRect)>,
    /// Live overlay rects while dragging.
    pub overlay_live: std::collections::HashMap<String, FracRect>,
    /// The theme draft (`theme.json`), saved on every Customize change.
    pub theme: fthrust_core::theme::ThemeStore,
    /// Open Customize sections.
    pub customize_open: [bool; 6],
    /// Export the theme as a ZIP (plan 5 opens the save picker).
    pub theme_export_requested: bool,
    /// Preview a notification sound (plan 5 plays it).
    pub sound_preview_requested: Option<String>,
    /// Open the export jobs dialog (plan 6).
    pub export_jobs_requested: bool,
    /// Keyboard navigation bindings.
    pub nav: fthrust_core::navigation::NavigationBindings,
    /// Last binding error shown on the Shortcuts page.
    pub shortcut_error: String,
    /// Action currently recording a navigation chord.
    pub shortcut_recording: Option<String>,
    /// Keys collected for the chord so far.
    pub shortcut_pending: Vec<String>,
    /// Keys currently held.
    pub shortcut_down: std::collections::BTreeSet<String>,
    /// The clips library (plan 5).
    pub library: crate::library::model::Library,
    /// Per-clip readiness.
    pub readiness: fthrust_core::clip_readiness::ClipReadinessRegistry,
    /// Library scroll offset.
    pub library_scroll: f32,
    /// The keyboard-selected clip.
    pub selected_clip: Option<std::path::PathBuf>,
    /// Inline rename in progress: the clip and its draft name.
    pub rename_draft: Option<(std::path::PathBuf, String)>,
    /// Records changed: rebuild the sections on the next frame.
    pub library_dirty: bool,
    /// Cards built last frame (visible rows plus overscan), in order.
    pub visible_cards: Vec<std::path::PathBuf>,
    /// Thumbnail jobs requested so far (tests).
    pub thumb_requests_total: usize,
    /// The thumbnail worker pool (started by the app; `None` in tests).
    pub thumb_pool: Option<crate::library::thumbs::ThumbPool>,
    /// Live thumbnail textures.
    pub thumb_textures: crate::library::thumbs::ThumbTextures,
    /// Metadata text and duration per clip once probed.
    pub thumb_meta: std::collections::HashMap<std::path::PathBuf, (String, Option<f64>)>,
    /// Clips whose thumbnail could not be made.
    pub thumb_failed: std::collections::HashSet<std::path::PathBuf>,
    /// Clips whose thumbnail was requested this generation.
    pub thumb_requested: std::collections::HashSet<std::path::PathBuf>,
    /// Transient card feedback: text and the time it expires.
    pub card_feedback: std::collections::HashMap<std::path::PathBuf, (String, f64)>,
    /// Open a clip in the editor (plan 6 consumes).
    pub open_clip_requested: Option<std::path::PathBuf>,
    /// The active clip editor; while set it covers the window.
    pub editor: Option<crate::editor::Editor>,
    /// The active editor's video frame texture.
    pub editor_video: Option<crate::video_texture::VideoTexture>,
    /// The active editor's filmstrip frames.
    pub editor_filmstrip: Vec<egui::TextureHandle>,
    /// Clip tags, descriptions and editor drafts (opened on first use).
    pub clip_metadata: Option<fthrust_core::clip_metadata::ClipMetadataStore>,
    /// Origin of the editor's clock (`editor_now`).
    pub editor_clock: std::time::Instant,
    /// Open a screenshot (plan 6 consumes).
    pub open_screenshot_requested: Option<std::path::PathBuf>,
    /// Upload a clip (plan 7 consumes).
    pub upload_requested: Option<std::path::PathBuf>,
    /// Show a file in the file manager (plan 5 services consume).
    pub show_in_file_manager_requested: Option<std::path::PathBuf>,
    /// Scrolling settled: the last wheel/drag time.
    pub library_last_scroll: f64,
    /// The live diagnostics session (events, report export).
    pub diagnostics: Option<std::sync::Arc<fthrust_core::diagnostics::session::DiagnosticsSession>>,
    /// The clips-folder-unavailable warning was shown.
    pub storage_error_reported: bool,
    /// A second launch asked for the window (the shell focuses it).
    pub activation_requested: bool,
    /// The engine is connected (Record and Save are allowed).
    pub engine_connected: bool,
    /// Cards to show (the overlay drains them).
    pub pending_cards: Vec<crate::services::CardKind>,
    /// Sound cues to play (the sound service drains them).
    pub pending_sounds: Vec<String>,
    /// Engine events for the recording service (drained each tick).
    pub pending_engine_events: Vec<fthrust_ipc::Event>,
    /// A screenshot hotkey fired.
    pub screenshot_requested: bool,
    /// The Record button text (`Record`, `Preparing…`, `Stop  hh:mm:ss`, …).
    pub record_label: String,
    /// The Record button uses the active (red) style.
    pub record_active: bool,
    /// The last Hyprland bind line copied from the hotkey popup.
    pub last_copied_bind_line: Option<String>,
    /// The compositor exposes its toplevel list (game detection works).
    pub game_detection_available: bool,
    /// The startup cue played (once per run).
    pub startup_cue_played: bool,
    /// Confirm the detected game.
    pub game_confirm_requested: bool,
    /// Dismiss the detected game.
    pub game_dismiss_requested: bool,
    /// Text being edited in an input dialog.
    pub input_draft: Option<String>,
    /// Minutes being edited in the custom clip length dialog.
    pub clip_length_draft: Option<f64>,
    /// Export preset form draft.
    pub export_preset_draft: Option<crate::dialogs::export_preset::Draft>,
    /// The dialog generation the drafts belong to.
    pub draft_generation: u64,
}

/// An overlay rect as canvas fractions `(x, y, w, h)`.
pub type FracRect = (f64, f64, f64, f64);

/// Which folder a BROWSE button asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowseKind {
    /// Clips storage folder.
    ClipsFolder,
    /// Manual recording folder.
    RecordingFolder,
    /// A folder to import clips from.
    ImportFolder,
    /// Gary Mode image.
    GaryImage,
    /// Image overlay layers.
    ImageLayers,
    /// Overlay preview background image.
    PreviewBackground,
    /// A theme ZIP to import.
    ThemeArchive,
    /// A font file for a role (`display` / `body`).
    FontFile(String),
    /// A custom icon file for a logical icon.
    IconFile(String),
    /// A custom sound file for a sound key.
    SoundFile(String),
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
        let clips_root = settings
            .clips_directory()
            .unwrap_or_else(|_| paths.default_clips_directory());
        let theme = fthrust_core::theme::ThemeStore::load(paths.clone())?;
        let nav = fthrust_core::navigation::NavigationBindings::from_settings(&settings);
        let client_exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "fthrust".into());
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
            client_exe,
            compositor: crate::popups::hotkeys::detect_compositor(),
            running_windows: Vec::new(),
            running_window_index: 0,
            capture_profile: CaptureProfile::Clips,
            restart_pending: false,
            source_restart_pending: false,
            error_action_requested: None,
            engine_restart_requested: false,
            refresh_windows_requested: false,
            open_desktop_shortcuts_requested: false,
            tab_slide: Default::default(),
            encoder_capabilities: None,
            encoder_from_cache: false,
            active_encoder: None,
            encoder_dirty: false,
            redetect_encoders_requested: false,
            browse_requested: None,
            scan_clips_requested: false,
            clips_directory_changed: false,
            imported_folders_changed: false,
            diagnostic_export_requested: false,
            diagnostic_report_zip: None,
            export_preset_selected: String::new(),
            settings_preset_selected: String::new(),
            status_hint: None,
            mic_level: None,
            gary_drag: None,
            mic_devices: Vec::new(),
            audio_devices: Vec::new(),
            camera_devices: Vec::new(),
            refresh_audio_devices_requested: false,
            refresh_cameras_requested: false,
            selected_image_layer: 0,
            overlay_drag: None,
            overlay_live: std::collections::HashMap::new(),
            theme,
            customize_open: [false; 6],
            theme_export_requested: false,
            sound_preview_requested: None,
            export_jobs_requested: false,
            nav,
            shortcut_error: String::new(),
            shortcut_recording: None,
            shortcut_pending: Vec::new(),
            shortcut_down: std::collections::BTreeSet::new(),
            library: crate::library::model::Library::new(clips_root),
            readiness: fthrust_core::clip_readiness::ClipReadinessRegistry::new(),
            library_scroll: 0.0,
            selected_clip: None,
            rename_draft: None,
            library_dirty: true,
            visible_cards: Vec::new(),
            thumb_requests_total: 0,
            thumb_pool: None,
            thumb_textures: crate::library::thumbs::ThumbTextures::default(),
            thumb_meta: std::collections::HashMap::new(),
            thumb_failed: std::collections::HashSet::new(),
            thumb_requested: std::collections::HashSet::new(),
            card_feedback: std::collections::HashMap::new(),
            open_clip_requested: None,
            editor: None,
            editor_video: None,
            editor_filmstrip: Vec::new(),
            clip_metadata: None,
            editor_clock: std::time::Instant::now(),
            open_screenshot_requested: None,
            upload_requested: None,
            show_in_file_manager_requested: None,
            library_last_scroll: -10.0,
            diagnostics: None,
            storage_error_reported: false,
            activation_requested: false,
            engine_connected: false,
            pending_cards: Vec::new(),
            pending_sounds: Vec::new(),
            pending_engine_events: Vec::new(),
            screenshot_requested: false,
            record_label: "Record".into(),
            record_active: false,
            last_copied_bind_line: None,
            game_detection_available: true,
            startup_cue_played: false,
            game_confirm_requested: false,
            game_dismiss_requested: false,
            input_draft: None,
            clip_length_draft: None,
            export_preset_draft: None,
            draft_generation: 0,
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
        // A client path whose bind command is as wide as the reference's
        // `echo | nc -U` line, so the Qt popup references still apply.
        state.client_exe = "/opt/fthrust/bin/xxxxxxxxxxxxxxxxxxxxx/fthrust".into();
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

impl AppState {
    /// Seconds on the editor's clock: every editor timer and UI event uses it.
    pub fn editor_now(&self) -> f64 {
        self.editor_clock.elapsed().as_secs_f64()
    }

    /// Ticks the active editor (timers, player, draft) and uploads its
    /// newest video frame when a render state exists.
    pub fn tick_editor(&mut self, render_state: Option<&egui_wgpu::RenderState>) {
        if self.editor.is_none() {
            return;
        }
        if self.clip_metadata.is_none() {
            match fthrust_core::clip_metadata::ClipMetadataStore::open(&self.paths) {
                Ok(store) => self.clip_metadata = Some(store),
                Err(e) => {
                    tracing::warn!(error = %e, "clip metadata store unavailable");
                    return;
                }
            }
        }
        let now = self.editor_now();
        let (Some(editor), Some(store)) = (self.editor.as_mut(), self.clip_metadata.as_mut())
        else {
            return;
        };
        editor.tick(now, &mut self.settings, store);
        if let (Some(rs), Some(frame)) = (
            render_state,
            editor.player_mut().and_then(|p| p.take_frame()),
        ) {
            self.editor_video
                .get_or_insert_with(crate::video_texture::VideoTexture::new)
                .update(rs, &frame);
        }
    }
}
