//! `scrollbar_qss`: 8 px bars, BG track, BORDER_HI r4 handle (min 32,
//! hover ACCENT), no arrows. The scroll style lives in
//! `tokens::install_style`; these helpers build areas with the right
//! identity and behaviour.

use egui::{ScrollArea, Ui};

use crate::tokens::Tokens;

/// A vertical scroll area named `name` (Qt `SmoothScrollArea`).
pub fn vertical(_ui: &Ui, _t: &Tokens, name: &str) -> ScrollArea {
    ScrollArea::vertical()
        .id_salt(name)
        .auto_shrink([false, false])
        .animated(true)
}

/// A horizontal-only scroll area (the capture toolbar).
pub fn horizontal(_ui: &Ui, _t: &Tokens, name: &str) -> ScrollArea {
    ScrollArea::horizontal()
        .id_salt(name)
        .auto_shrink([false, false])
}
