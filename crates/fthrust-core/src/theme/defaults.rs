//! Shipped theme tables, copied from the Python `theme_manager.py`.

use std::collections::BTreeMap;

/// The 25 colour tokens, in fixture order.
pub const COLOR_TOKENS: [&str; 25] = [
    "ACCENT",
    "ACCENT_DIM",
    "ACCENT_SOFT",
    "BG",
    "BORDER",
    "BORDER_HI",
    "CARD_BG",
    "CARD_BG_HI",
    "CARD_BORDER",
    "DELETE",
    "ERROR",
    "ERROR_SOFT",
    "HAIRLINE",
    "SHELL_BG",
    "SHELL_BG_2",
    "SHELL_DIVIDER",
    "SUCCESS",
    "SURFACE_1",
    "SURFACE_2",
    "SURFACE_3",
    "TEXT",
    "TEXT_DIM",
    "TEXT_GHOST",
    "TEXT_MUTED",
    "WARNING",
];

/// Current default colours, `(token, #rrggbb)`.
pub(super) const DEFAULT_COLOR_TABLE: &[(&str, &str)] = &[
    ("ACCENT", "#20e3a0"),
    ("ACCENT_DIM", "#16bf87"),
    ("ACCENT_SOFT", "#0d2b20"),
    ("BG", "#070a09"),
    ("BORDER", "#242c27"),
    ("BORDER_HI", "#49584f"),
    ("CARD_BG", "#101512"),
    ("CARD_BG_HI", "#141b17"),
    ("CARD_BORDER", "#1b231e"),
    ("DELETE", "#b84f45"),
    ("ERROR", "#ff795d"),
    ("ERROR_SOFT", "#270b0f"),
    ("HAIRLINE", "#181f1b"),
    ("SHELL_BG", "#0a0f0d"),
    ("SHELL_BG_2", "#101512"),
    ("SHELL_DIVIDER", "#1b231e"),
    ("SUCCESS", "#20e3a0"),
    ("SURFACE_1", "#0a0f0d"),
    ("SURFACE_2", "#101512"),
    ("SURFACE_3", "#141b17"),
    ("TEXT", "#f3f6f4"),
    ("TEXT_DIM", "#a0aba5"),
    ("TEXT_GHOST", "#7e8a83"),
    ("TEXT_MUTED", "#7e8a83"),
    ("WARNING", "#e7b56e"),
];

/// Current default capture-card colours.
pub(super) const DEFAULT_CAPTURE_CARD_TABLE: &[(&str, &str)] = &[
    ("CAPTURE_CARD_ACCENT", "#20e3a0"),
    ("CAPTURE_CARD_BG", "#0a0f0d"),
    ("CAPTURE_CARD_DIVIDER", "#242c27"),
    ("CAPTURE_CARD_PROGRESS_FILL", "#20e3a0"),
    ("CAPTURE_CARD_PROGRESS_TRACK", "#101512"),
    ("CAPTURE_CARD_STATS_DIM", "#a0aba5"),
    ("CAPTURE_CARD_TEXT", "#f3f6f4"),
];

/// Default font families per role.
pub(super) const DEFAULT_FONT_TABLE: &[(&str, &str)] = &[("body", "Oswald"), ("display", "Oswald")];

/// Default global icon tint.
pub const DEFAULT_ICON_TINT: &str = "#a0aba5";

/// Customisable icons, `(logical file name, label)`.
pub const CUSTOMIZABLE_ICONS: &[(&str, &str)] = &[
    ("camera.png", "Capture Camera"),
    ("clip.png", "Clip Tab"),
    ("close.png", "Close Button"),
    ("decrease.png", "Decrease"),
    ("delete.png", "Delete"),
    ("dropdown.png", "Dropdown Arrow"),
    ("favicon.ico", "App Icon"),
    ("home.png", "Home Button"),
    ("increase.png", "Increase"),
    ("link.png", "Copy Link"),
    ("maximize.png", "Maximize Button"),
    ("minimize.png", "Minimize Button"),
    ("pause.png", "Pause Button"),
    ("performance.png", "Performance Tab"),
    ("personalize.png", "Customize Tab"),
    ("play.png", "Play Button"),
    ("refresh.png", "Refresh Button"),
    ("settings(general).png", "General Tab"),
    ("shortcuts.png", "Shortcuts Tab"),
    ("shutdown.png", "Shutdown Button"),
    ("sound.png", "Audio Tab"),
    ("step-back.png", "Step Backward"),
    ("step-forward.png", "Step Forward"),
    ("updates.png", "Updates Tab"),
    ("upload.png", "Upload"),
    ("visuals.png", "Visuals Tab"),
];

/// Customisable sound keys.
pub const CUSTOMIZABLE_SOUNDS: [&str; 6] = [
    "clip_captured",
    "error",
    "screenshot_captured",
    "startup",
    "upload_failed",
    "upload_successful",
];

/// Customisable sounds with labels, `(key, label)`.
pub const CUSTOMIZABLE_SOUND_LABELS: &[(&str, &str)] = &[
    ("clip_captured", "Clip Captured"),
    ("error", "Error"),
    ("screenshot_captured", "Screenshot Captured"),
    ("startup", "Startup"),
    ("upload_failed", "Upload Failed"),
    ("upload_successful", "Upload Successful"),
];

/// Colour groups, `(group label, tokens)`.
pub const COLOR_GROUPS: &[(&str, &[&str])] = &[
    (
        "Surfaces",
        &[
            "BG",
            "SURFACE_1",
            "SURFACE_2",
            "SURFACE_3",
            "SHELL_BG",
            "SHELL_BG_2",
            "SHELL_DIVIDER",
        ],
    ),
    ("Accent", &["ACCENT", "ACCENT_DIM", "ACCENT_SOFT"]),
    ("Text", &["TEXT", "TEXT_DIM", "TEXT_MUTED", "TEXT_GHOST"]),
    ("Borders", &["BORDER", "BORDER_HI", "HAIRLINE"]),
    ("Cards", &["CARD_BG", "CARD_BG_HI", "CARD_BORDER"]),
    (
        "Status",
        &["ERROR", "ERROR_SOFT", "WARNING", "SUCCESS", "DELETE"],
    ),
];

/// Display label per colour token, in group order.
pub const COLOR_LABELS: &[(&str, &str)] = &[
    ("BG", "Background"),
    ("SURFACE_1", "Surface 1"),
    ("SURFACE_2", "Surface 2"),
    ("SURFACE_3", "Surface 3"),
    ("SHELL_BG", "Shell BG"),
    ("SHELL_BG_2", "Shell Secondary"),
    ("SHELL_DIVIDER", "Shell Line"),
    ("ACCENT", "Accent"),
    ("ACCENT_DIM", "Accent Dim"),
    ("ACCENT_SOFT", "Accent Soft"),
    ("TEXT", "Primary"),
    ("TEXT_DIM", "Secondary"),
    ("TEXT_MUTED", "Muted"),
    ("TEXT_GHOST", "Ghost"),
    ("BORDER", "Border"),
    ("BORDER_HI", "Border Light"),
    ("HAIRLINE", "Hairline"),
    ("CARD_BG", "Card BG"),
    ("CARD_BG_HI", "Card Hover"),
    ("CARD_BORDER", "Card Border"),
    ("ERROR", "Error"),
    ("ERROR_SOFT", "Error Soft"),
    ("WARNING", "Warning"),
    ("SUCCESS", "Success"),
    ("DELETE", "Delete Button"),
];

/// Logical icon name to bundled SVG.
pub const ICON_ASSETS: &[(&str, &str)] = &[
    ("camera.png", "camera.svg"),
    ("clip.png", "clips.svg"),
    ("close.png", "close.svg"),
    ("decrease.png", "minimise.svg"),
    ("delete.png", "delete.svg"),
    ("dropdown.png", "dropdown.svg"),
    ("home.png", "home.svg"),
    ("increase.png", "plus.svg"),
    ("link.png", "copylink.svg"),
    ("maximize.png", "maximise.svg"),
    ("minimize.png", "minimise.svg"),
    ("pause.png", "pause.svg"),
    ("performance.png", "performance.svg"),
    ("personalize.png", "customise.svg"),
    ("play.png", "play.svg"),
    ("refresh.png", "refresh.svg"),
    ("settings(general).png", "settings.svg"),
    ("shortcuts.png", "shortcuts.svg"),
    ("shutdown.png", "shutdown.svg"),
    ("sound.png", "sound.svg"),
    ("step-back.png", "chevronleft.svg"),
    ("step-forward.png", "chevronright.svg"),
    ("updates.png", "update.svg"),
    ("upload.png", "upload.svg"),
    ("visuals.png", "visuals.svg"),
];

const LEGACY_V1: &[(&str, &str)] = &[
    ("ACCENT", "#00ffaa"),
    ("ACCENT_DIM", "#00aa72"),
    ("ACCENT_SOFT", "#0a2218"),
    ("BG", "#000000"),
    ("BORDER", "#222222"),
    ("BORDER_HI", "#333333"),
    ("CARD_BG", "#0a0a0a"),
    ("CARD_BG_HI", "#111111"),
    ("CARD_BORDER", "#222222"),
    ("DELETE", "#cc0000"),
    ("ERROR", "#cc0000"),
    ("ERROR_SOFT", "#240606"),
    ("HAIRLINE", "#111111"),
    ("SHELL_BG", "#000000"),
    ("SHELL_BG_2", "#0a0a0a"),
    ("SHELL_DIVIDER", "#222222"),
    ("SUCCESS", "#00aa00"),
    ("SURFACE_1", "#0a0a0a"),
    ("SURFACE_2", "#111111"),
    ("SURFACE_3", "#1c1c1c"),
    ("TEXT", "#ffffff"),
    ("TEXT_DIM", "#888888"),
    ("TEXT_GHOST", "#222222"),
    ("TEXT_MUTED", "#555555"),
    ("WARNING", "#E8871A"),
];

const LEGACY_V2: &[(&str, &str)] = &[
    ("ACCENT", "#00ffaa"),
    ("ACCENT_DIM", "#62e8bb"),
    ("ACCENT_SOFT", "#2a3634"),
    ("BG", "#242629"),
    ("BORDER", "#45494d"),
    ("BORDER_HI", "#89938f"),
    ("CARD_BG", "#2c2f33"),
    ("CARD_BG_HI", "#35393d"),
    ("CARD_BORDER", "#45494d"),
    ("DELETE", "#9e444d"),
    ("ERROR", "#ff8585"),
    ("ERROR_SOFT", "#502b30"),
    ("HAIRLINE", "#35393d"),
    ("SHELL_BG", "#202225"),
    ("SHELL_BG_2", "#202326"),
    ("SHELL_DIVIDER", "#45494d"),
    ("SUCCESS", "#00ffaa"),
    ("SURFACE_1", "#202326"),
    ("SURFACE_2", "#2c2f33"),
    ("SURFACE_3", "#35393d"),
    ("TEXT", "#f3f5f4"),
    ("TEXT_DIM", "#b9bfbe"),
    ("TEXT_GHOST", "#b9bfbe"),
    ("TEXT_MUTED", "#adb5b2"),
    ("WARNING", "#ffc078"),
];

const LEGACY_V3: &[(&str, &str)] = &[
    ("ACCENT", "#63cfb0"),
    ("ACCENT_DIM", "#49b696"),
    ("ACCENT_SOFT", "#203731"),
    ("BG", "#14171b"),
    ("BORDER", "#353e48"),
    ("BORDER_HI", "#64717e"),
    ("CARD_BG", "#1e2329"),
    ("CARD_BG_HI", "#262c34"),
    ("CARD_BORDER", "#303740"),
    ("DELETE", "#b45d68"),
    ("ERROR", "#ef9197"),
    ("ERROR_SOFT", "#3b232a"),
    ("HAIRLINE", "#252b33"),
    ("SHELL_BG", "#0f1216"),
    ("SHELL_BG_2", "#171b20"),
    ("SHELL_DIVIDER", "#2c323a"),
    ("SUCCESS", "#63cfb0"),
    ("SURFACE_1", "#191d22"),
    ("SURFACE_2", "#22272e"),
    ("SURFACE_3", "#2c333b"),
    ("TEXT", "#edf0f3"),
    ("TEXT_DIM", "#b4bdc7"),
    ("TEXT_GHOST", "#929eab"),
    ("TEXT_MUTED", "#929eab"),
    ("WARNING", "#dfb77c"),
];

const LEGACY_CC_V1: &[(&str, &str)] = &[
    ("CAPTURE_CARD_ACCENT", "#ffffff"),
    ("CAPTURE_CARD_BG", "#000000"),
    ("CAPTURE_CARD_DIVIDER", "#222222"),
    ("CAPTURE_CARD_PROGRESS_FILL", "#ffffff"),
    ("CAPTURE_CARD_PROGRESS_TRACK", "#111111"),
    ("CAPTURE_CARD_STATS_DIM", "#666666"),
    ("CAPTURE_CARD_TEXT", "#ffffff"),
];

const LEGACY_CC_V2: &[(&str, &str)] = &[
    ("CAPTURE_CARD_ACCENT", "#00ffaa"),
    ("CAPTURE_CARD_BG", "#202326"),
    ("CAPTURE_CARD_DIVIDER", "#45494d"),
    ("CAPTURE_CARD_PROGRESS_FILL", "#00ffaa"),
    ("CAPTURE_CARD_PROGRESS_TRACK", "#2c2f33"),
    ("CAPTURE_CARD_STATS_DIM", "#b9bfbe"),
    ("CAPTURE_CARD_TEXT", "#f3f5f4"),
];

const LEGACY_CC_V3: &[(&str, &str)] = &[
    ("CAPTURE_CARD_ACCENT", "#63cfb0"),
    ("CAPTURE_CARD_BG", "#191d22"),
    ("CAPTURE_CARD_DIVIDER", "#353e48"),
    ("CAPTURE_CARD_PROGRESS_FILL", "#63cfb0"),
    ("CAPTURE_CARD_PROGRESS_TRACK", "#22272e"),
    ("CAPTURE_CARD_STATS_DIM", "#b4bdc7"),
    ("CAPTURE_CARD_TEXT", "#edf0f3"),
];

fn to_map(table: &[(&str, &str)]) -> BTreeMap<String, String> {
    table
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// Current default colours keyed by token.
pub fn default_colors() -> BTreeMap<String, String> {
    to_map(DEFAULT_COLOR_TABLE)
}

/// Current default capture-card colours.
pub fn default_capture_card_colors() -> BTreeMap<String, String> {
    to_map(DEFAULT_CAPTURE_CARD_TABLE)
}

/// Default font family per role (`body`, `display`).
pub fn default_fonts() -> BTreeMap<String, String> {
    to_map(DEFAULT_FONT_TABLE)
}

/// Shipped palette of an earlier version (1, 2 or 3); empty for other values.
pub fn legacy_palette(version: u8) -> BTreeMap<String, String> {
    match version {
        1 => to_map(LEGACY_V1),
        2 => to_map(LEGACY_V2),
        3 => to_map(LEGACY_V3),
        _ => BTreeMap::new(),
    }
}

/// Shipped capture-card palette of an earlier version (1, 2 or 3).
pub fn legacy_capture_card(version: u8) -> BTreeMap<String, String> {
    match version {
        1 => to_map(LEGACY_CC_V1),
        2 => to_map(LEGACY_CC_V2),
        3 => to_map(LEGACY_CC_V3),
        _ => BTreeMap::new(),
    }
}
