//! The single source of colours, type sizes, geometry and motion timings
//! (`docs/reference/reference-ui-inventory.md` section 2). Widgets read
//! tokens; nothing else holds a colour or size literal.

use std::collections::BTreeMap;
use std::path::PathBuf;

use egui::Color32;

/// The 25 palette tokens, in `theme_manager.py` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum Color {
    Bg,
    Surface1,
    Surface2,
    Surface3,
    ShellBg,
    ShellBg2,
    ShellDivider,
    CardBg,
    CardBgHi,
    CardBorder,
    Hairline,
    Border,
    BorderHi,
    Text,
    TextDim,
    TextMuted,
    TextGhost,
    Accent,
    AccentDim,
    AccentSoft,
    Error,
    ErrorSoft,
    Warning,
    Success,
    Delete,
}

impl Color {
    /// Every token, in palette order.
    pub const ALL: [Color; 25] = [
        Color::Bg,
        Color::Surface1,
        Color::Surface2,
        Color::Surface3,
        Color::ShellBg,
        Color::ShellBg2,
        Color::ShellDivider,
        Color::CardBg,
        Color::CardBgHi,
        Color::CardBorder,
        Color::Hairline,
        Color::Border,
        Color::BorderHi,
        Color::Text,
        Color::TextDim,
        Color::TextMuted,
        Color::TextGhost,
        Color::Accent,
        Color::AccentDim,
        Color::AccentSoft,
        Color::Error,
        Color::ErrorSoft,
        Color::Warning,
        Color::Success,
        Color::Delete,
    ];

    /// The `theme.json` key.
    pub fn key(self) -> &'static str {
        match self {
            Color::Bg => "BG",
            Color::Surface1 => "SURFACE_1",
            Color::Surface2 => "SURFACE_2",
            Color::Surface3 => "SURFACE_3",
            Color::ShellBg => "SHELL_BG",
            Color::ShellBg2 => "SHELL_BG_2",
            Color::ShellDivider => "SHELL_DIVIDER",
            Color::CardBg => "CARD_BG",
            Color::CardBgHi => "CARD_BG_HI",
            Color::CardBorder => "CARD_BORDER",
            Color::Hairline => "HAIRLINE",
            Color::Border => "BORDER",
            Color::BorderHi => "BORDER_HI",
            Color::Text => "TEXT",
            Color::TextDim => "TEXT_DIM",
            Color::TextMuted => "TEXT_MUTED",
            Color::TextGhost => "TEXT_GHOST",
            Color::Accent => "ACCENT",
            Color::AccentDim => "ACCENT_DIM",
            Color::AccentSoft => "ACCENT_SOFT",
            Color::Error => "ERROR",
            Color::ErrorSoft => "ERROR_SOFT",
            Color::Warning => "WARNING",
            Color::Success => "SUCCESS",
            Color::Delete => "DELETE",
        }
    }

    /// The token for a `theme.json` key.
    pub fn from_key(key: &str) -> Option<Color> {
        Color::ALL.iter().copied().find(|c| c.key() == key)
    }
}

/// Parses `#rrggbb`.
pub fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim();
    if s.len() != 7 || !s.starts_with('#') {
        return None;
    }
    let v = u32::from_str_radix(&s[1..], 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Formats as `#rrggbb`.
pub fn to_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// The capture-card colour group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct CaptureCardColors {
    pub bg: Color32,
    pub accent: Color32,
    pub text: Color32,
    pub divider: Color32,
    pub stats_dim: Color32,
    pub progress_track: Color32,
    pub progress_fill: Color32,
}

/// Everything a widget may read.
#[derive(Clone, Debug, PartialEq)]
pub struct Tokens {
    /// Palette, indexed by `Color as usize`.
    pub colors: [Color32; 25],
    /// Capture-card group.
    pub capture_card: CaptureCardColors,
    /// Display font family name.
    pub display_family: String,
    /// Body font family name.
    pub body_family: String,
    /// `_global` icon tint.
    pub icon_tint_global: Color32,
    /// Per-icon tints by logical name (`play.png`).
    pub icon_tints: BTreeMap<String, Color32>,
    /// Custom icon files by logical name.
    pub icon_overrides: BTreeMap<String, PathBuf>,
    /// Custom font files by family name.
    pub font_files: Vec<(String, PathBuf)>,
    /// Running under Wayland (popup fades are skipped).
    pub wayland: bool,
    /// `reduce_ui_animations`.
    pub reduce_motion: bool,
}

impl Default for Tokens {
    fn default() -> Self {
        Self::default_palette()
    }
}

impl Tokens {
    /// The shipped palette (v4), Oswald, TEXT_DIM tint.
    pub fn default_palette() -> Tokens {
        let defaults = fthrust_core::theme::default_colors();
        let mut colors = [Color32::BLACK; 25];
        for (i, c) in Color::ALL.iter().enumerate() {
            colors[i] = defaults
                .get(c.key())
                .and_then(|v| parse_hex(v))
                .unwrap_or(Color32::MAGENTA);
        }
        let cc = fthrust_core::theme::default_capture_card_colors();
        let pick = |k: &str| {
            cc.get(k)
                .and_then(|v| parse_hex(v))
                .unwrap_or(Color32::MAGENTA)
        };
        Tokens {
            colors,
            capture_card: CaptureCardColors {
                bg: pick("CAPTURE_CARD_BG"),
                accent: pick("CAPTURE_CARD_ACCENT"),
                text: pick("CAPTURE_CARD_TEXT"),
                divider: pick("CAPTURE_CARD_DIVIDER"),
                stats_dim: pick("CAPTURE_CARD_STATS_DIM"),
                progress_track: pick("CAPTURE_CARD_PROGRESS_TRACK"),
                progress_fill: pick("CAPTURE_CARD_PROGRESS_FILL"),
            },
            display_family: "Oswald".into(),
            body_family: "Oswald".into(),
            icon_tint_global: parse_hex(fthrust_core::theme::DEFAULT_ICON_TINT)
                .unwrap_or(Color32::GRAY),
            icon_tints: BTreeMap::new(),
            icon_overrides: BTreeMap::new(),
            font_files: Vec::new(),
            wayland: std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
            reduce_motion: false,
        }
    }

    /// A palette colour.
    pub fn color(&self, c: Color) -> Color32 {
        self.colors[c as usize]
    }

    /// Sets a palette colour.
    pub fn set_color(&mut self, c: Color, value: Color32) {
        self.colors[c as usize] = value;
    }

    /// Title-strip background (= SHELL_BG).
    pub fn bar_bg(&self) -> Color32 {
        self.color(Color::ShellBg)
    }

    /// Title-strip foreground (= TEXT).
    pub fn bar_fg(&self) -> Color32 {
        self.color(Color::Text)
    }

    /// Title-strip dim foreground (= TEXT_DIM).
    pub fn bar_fg_dim(&self) -> Color32 {
        self.color(Color::TextDim)
    }

    /// Title-strip hairline (= SHELL_DIVIDER).
    pub fn bar_hairline(&self) -> Color32 {
        self.color(Color::ShellDivider)
    }

    /// The tint for a logical icon name (per-icon else global).
    pub fn icon_tint(&self, logical: &str) -> Color32 {
        self.icon_tints
            .get(logical)
            .copied()
            .unwrap_or(self.icon_tint_global)
    }
}

/// Pixel sizes (`ui/style.py` `Fonts.SIZE_*`).
pub mod size {
    /// 12 px.
    pub const MICRO: f32 = 12.0;
    /// 14 px.
    pub const LABEL: f32 = 14.0;
    /// 15 px, the application default.
    pub const BODY: f32 = 15.0;
    /// 17 px.
    pub const BODY_L: f32 = 17.0;
    /// 15 px.
    pub const TOOLBAR: f32 = 15.0;
    /// 20 px.
    pub const H3: f32 = 20.0;
    /// 26 px.
    pub const H2: f32 = 26.0;
    /// 36 px.
    pub const H1: f32 = 36.0;
}

/// Geometry tokens (`ui/style.py` `Sizes`).
pub mod geom {
    /// Title strip height.
    pub const TITLE_BAR_H: f32 = 36.0;
    /// Capture toolbar height.
    pub const CAPTURE_BAR_H: f32 = 64.0;
    /// Toolbar button height.
    pub const CAPTURE_CONTROL_H: f32 = 44.0;
    /// Clips filter bar height.
    pub const FILTER_H: f32 = 60.0;
    /// Standard button height.
    pub const BUTTON_H: f32 = 38.0;
    /// Standard input height.
    pub const INPUT_H: f32 = 38.0;
    /// Error bar height.
    pub const HW_BAR_H: f32 = 36.0;
    /// Navigation rail width.
    pub const NAV_W: f32 = 94.0;
    /// Navigation button side.
    pub const NAV_BTN: f32 = 78.0;
    /// Default corner radius.
    pub const RADIUS: f32 = 6.0;
    /// Small radius.
    pub const RADIUS_SM: f32 = 4.0;
    /// Card radius.
    pub const RADIUS_CARD: f32 = 8.0;
    /// Page inset.
    pub const PAGE_INSET: f32 = 24.0;
    /// Row gap.
    pub const ROW_GAP: f32 = 12.0;
    /// Section gap.
    pub const SECTION_GAP: f32 = 24.0;
    /// Invisible resize border.
    pub const RESIZE_MARGIN: f32 = 8.0;
    /// SPACE_2..SPACE_9.
    pub const SPACE: [f32; 8] = [4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 32.0, 48.0];
    /// Editor sidebar width.
    pub const EDITOR_SIDEBAR_W: f32 = 280.0;
}

/// Motion timings in milliseconds (`ui/motion.py`, section 2.5).
pub mod motion {
    /// Hover colour blend.
    pub const HOVER_MS: u64 = 120;
    /// Page, category and clip slides.
    pub const SLIDE_MS: u64 = 220;
    /// Popup fade (skipped on Wayland).
    pub const POPUP_FADE_MS: u64 = 180;
    /// Capture card slide in.
    pub const CARD_IN_MS: u64 = 450;
    /// Capture card hold.
    pub const CARD_HOLD_MS: u64 = 2400;
    /// Capture card slide out.
    pub const CARD_OUT_MS: u64 = 350;
    /// Smooth-scroll time constant.
    pub const SCROLL_TAU_MS: u64 = 35;
}
