//! `ClipGrid._layout_metrics`: columns and exact per-column widths for a
//! viewport, and the card geometry constants.

/// Preferred card width.
pub const CARD_W: f32 = 360.0;
/// Minimum card width.
pub const CARD_MIN_W: f32 = 300.0;
/// Maximum card width.
pub const CARD_MAX_W: f32 = 440.0;
/// Card body height below the 16:9 thumbnail.
pub const CARD_BODY_H: f32 = 88.0;
/// Grid spacing (horizontal and vertical).
pub const GRID_SPACING: f32 = 12.0;
/// Page side margins (24 + 24).
pub const H_MARGIN: f32 = 48.0;
/// Spacing between day sections.
pub const SECTION_SPACING: f32 = 20.0;

/// Column count and per-column widths for `viewport_width` (integer
/// arithmetic as in the reference; widths distribute the remainder left
/// to right).
pub fn metrics(viewport_width: f32) -> (usize, Vec<f32>) {
    let (min_w, max_w, w, sp, hm) = (
        CARD_MIN_W as i64,
        CARD_MAX_W as i64,
        CARD_W as i64,
        GRID_SPACING as i64,
        H_MARGIN as i64,
    );
    let available = (viewport_width as i64 - hm).max(min_w);
    let mut cols = ((available + sp) / (w + sp)).max(1);
    while cols > 1 {
        let card_width = (available - sp * (cols - 1)) / cols;
        if card_width >= min_w {
            break;
        }
        cols -= 1;
    }
    loop {
        let card_width = (available - sp * (cols - 1)) / cols;
        if card_width <= max_w {
            break;
        }
        if (available - sp * cols) / (cols + 1) < min_w {
            break;
        }
        cols += 1;
    }
    let usable = (available - sp * (cols - 1)).min(cols * max_w);
    let base = usable / cols;
    let remainder = usable % cols;
    let widths = (0..cols)
        .map(|c| (base + i64::from(c < remainder)) as f32)
        .collect();
    (cols as usize, widths)
}

/// Card height for a width: the 16:9 thumbnail plus the body.
pub fn card_height(width: f32) -> f32 {
    (width * 9.0 / 16.0).round() + CARD_BODY_H
}
