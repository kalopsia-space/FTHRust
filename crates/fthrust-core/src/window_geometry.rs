//! Window geometry persistence and restore in logical (Qt-style) coordinates.
//!
//! Saved rectangles are never rescaled; DPI values are diagnostic only.

use serde::{Deserialize, Serialize};

/// Minimum window width in logical pixels.
pub const MIN_WIDTH: u32 = 1100;
/// Minimum window height in logical pixels.
pub const MIN_HEIGHT: u32 = 720;
/// Minimum width of a saved rectangle for it to count as present (also the
/// reference's minimum useful visible width).
pub const MIN_VISIBLE_WIDTH: u32 = 64;
/// Minimum height of a saved rectangle for it to count as present.
pub const MIN_VISIBLE_HEIGHT: u32 = 40;
const POPUP_MARGIN: i64 = 8;

/// An integer rectangle in logical pixels.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// Window geometry as persisted in settings.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct SavedGeometry {
    /// Geometry of the window when not maximized.
    pub normal_geometry: Option<Rect>,
    /// Name of the screen the window was on.
    pub screen_name: Option<String>,
    /// Whether the window was maximized.
    pub maximized: Option<bool>,
    /// Device pixel ratio at save time (diagnostic only).
    pub saved_dpr: Option<f64>,
    /// Logical DPI at save time (diagnostic only).
    pub saved_logical_dpi: Option<f64>,
}

/// A connected screen and its available work area.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Screen {
    /// Screen name.
    pub name: String,
    /// Available (work-area) geometry.
    pub available: Rect,
    /// Whether this is the primary screen.
    pub primary: bool,
}

/// Resolved window placement.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Placement {
    /// Window rectangle to apply.
    pub rect: Rect,
    /// Name of the chosen screen.
    pub screen_name: String,
    /// Whether to maximize the window.
    pub maximized: bool,
}

fn overlap_area(a: Rect, b: Rect) -> i64 {
    let w = (i64::from(a.x) + i64::from(a.width)).min(i64::from(b.x) + i64::from(b.width))
        - i64::from(a.x).max(i64::from(b.x));
    let h = (i64::from(a.y) + i64::from(a.height)).min(i64::from(b.y) + i64::from(b.height))
        - i64::from(a.y).max(i64::from(b.y));
    w.max(0) * h.max(0)
}

fn clamp_axis(pos: i32, size: u32, start: i32, avail: u32) -> i32 {
    let max = i64::from(start) + i64::from(avail) - i64::from(size);
    i64::from(pos).min(max).max(i64::from(start)) as i32
}

fn clamp_rect(r: Rect, a: Rect) -> Rect {
    let width = r.width.max(MIN_WIDTH).min(a.width);
    let height = r.height.max(MIN_HEIGHT).min(a.height);
    Rect {
        x: clamp_axis(r.x, width, a.x, a.width),
        y: clamp_axis(r.y, height, a.y, a.height),
        width,
        height,
    }
}

/// Resolve where to place the window.
///
/// The screen is the saved one by name (case-insensitive), else the one with
/// the largest overlap with the saved rectangle, else the primary, else the
/// first. A saved rectangle under 64x40 counts as missing and is replaced by
/// a centred default. The result is clamped to the screen's work area.
/// Returns `None` only when `screens` is empty.
pub fn restore(saved: &SavedGeometry, screens: &[Screen]) -> Option<Placement> {
    let first = screens.first()?;
    let candidate = saved
        .normal_geometry
        .filter(|r| r.width >= MIN_VISIBLE_WIDTH && r.height >= MIN_VISIBLE_HEIGHT);
    let by_name = saved
        .screen_name
        .as_deref()
        .filter(|n| !n.is_empty())
        .and_then(|n| {
            screens
                .iter()
                .find(|s| s.name.to_lowercase() == n.to_lowercase())
        });
    let screen = by_name
        .or_else(|| {
            let c = candidate?;
            screens
                .iter()
                .map(|s| (overlap_area(c, s.available), s))
                .filter(|(area, _)| *area > 0)
                .fold(None, |best: Option<(i64, &Screen)>, cur| match best {
                    Some(b) if b.0 >= cur.0 => Some(b),
                    _ => Some(cur),
                })
                .map(|(_, s)| s)
        })
        .or_else(|| screens.iter().find(|s| s.primary))
        .unwrap_or(first);
    let a = screen.available;
    let rect = candidate.unwrap_or_else(|| {
        let width = MIN_WIDTH.min(a.width);
        let height = MIN_HEIGHT.min(a.height);
        Rect {
            x: a.x + ((a.width - width) / 2) as i32,
            y: a.y + ((a.height - height) / 2) as i32,
            width,
            height,
        }
    });
    Some(Placement {
        rect: clamp_rect(rect, a),
        screen_name: screen.name.clone(),
        maximized: saved.maximized.unwrap_or(true),
    })
}

/// Position a popup of `popup` (width, height) just below `anchor`, inside
/// `screen` shrunk by an 8 px margin on every side. Mirrors the reference:
/// if the popup overflows the right edge it is right-aligned to the anchor,
/// if it overflows the bottom edge it is flipped above the anchor, and the
/// result is finally clamped into the margined area. Returns the top-left.
pub fn popup_anchor(anchor: Rect, popup: (u32, u32), screen: Rect) -> (i32, i32) {
    let m = POPUP_MARGIN;
    let (sx, sy) = (i64::from(screen.x) + m, i64::from(screen.y) + m);
    let (sw, sh) = (
        i64::from(screen.width) - 2 * m,
        i64::from(screen.height) - 2 * m,
    );
    let width = i64::from(popup.0).max(1).min(sw.max(1));
    let height = i64::from(popup.1).max(1).min(sh.max(1));
    let mut x = i64::from(anchor.x);
    let mut y = i64::from(anchor.y) + i64::from(anchor.height);
    if x + width > sx + sw {
        x = i64::from(anchor.x) + i64::from(anchor.width) - width;
    }
    if y + height > sy + sh {
        y = i64::from(anchor.y) - height;
    }
    let x = x.max(sx).min(sx + sw - width);
    let y = y.max(sy).min(sy + sh - height);
    (x as i32, y as i32)
}
