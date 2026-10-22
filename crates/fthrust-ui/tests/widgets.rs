mod common;

use egui::{Pos2, Vec2};
use fthrust_ui::fonts::Weight;
use fthrust_ui::icons::{Icon, IconCache};
use fthrust_ui::tokens::{Color, Tokens};
use fthrust_ui::widgets::{buttons, check, input, section, slider, toggle};

const CANVAS: Vec2 = Vec2::new(400.0, 300.0);

fn tokens() -> Tokens {
    let mut t = Tokens::default_palette();
    t.reduce_motion = true; // widgets settle in one frame
    t
}

#[test]
fn primary_button_geometry_colors_and_states() {
    let t = tokens();
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 100.0),
            )),
            |ui| {
                buttons::primary(ui, &t, "New", "newBtn");
            },
        );
    });
    let m = r.marker("newBtn").unwrap();
    assert_eq!(m.rect.min, Pos2::new(20.0, 20.0));
    assert!(
        (m.rect.height() - 41.0).abs() <= 1.0,
        "Qt: 21 px text + 10 px padding ×2 = 41, got {}",
        m.rect.height()
    );
    assert!(
        (m.rect.width() - 60.0).abs() <= 1.5,
        "Qt 'New' primary is 60 wide, got {}",
        m.rect.width()
    );
    assert_eq!(m.text.as_ref().unwrap().px, 14.0);
    assert_eq!(m.text.as_ref().unwrap().weight, Weight::W600);
    assert_eq!(
        r.pixel(Pos2::new(28.0, 40.0)),
        Some(t.color(Color::Accent)),
        "fill at the left padding"
    );
    // Hover → TEXT fill; pressed → ACCENT_DIM.
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 100.0),
            )),
            |ui| {
                buttons::primary(ui, &t, "New", "newBtn");
            },
        );
    });
    h.hover_at(Pos2::new(40.0, 40.0));
    h.run_steps(3);
    let img = h.render().unwrap();
    let p = img.get_pixel(28, 40);
    assert_eq!((p[0], p[1], p[2]), (0xf3, 0xf6, 0xf4), "hover fill is TEXT");
    h.drag_at(Pos2::new(40.0, 40.0));
    h.run_steps(2);
    let img = h.render().unwrap();
    let p = img.get_pixel(28, 40);
    assert_eq!(
        (p[0], p[1], p[2]),
        (0x16, 0xbf, 0x87),
        "pressed fill is ACCENT_DIM"
    );
}

#[test]
fn outline_and_secondary_buttons_match_qt_sizes() {
    let t = tokens();
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 200.0),
            )),
            |ui| {
                buttons::outline(ui, &t, "Delete", "deleteBtn");
                ui.add_space(20.0);
                buttons::secondary(ui, &t, "Delete", "secondaryBtn");
                ui.add_space(20.0);
                buttons::pill(ui, &t, "Delete", "pillBtn");
                ui.add_space(20.0);
                buttons::ghost(ui, &t, "Delete", "ghostBtn");
            },
        );
    });
    let outline = r.marker("deleteBtn").unwrap();
    assert!(
        (outline.rect.width() - 70.0).abs() <= 1.5,
        "Qt 'Delete' outline is 70 wide: {}",
        outline.rect.width()
    );
    assert!((outline.rect.height() - 41.0).abs() <= 1.0);
    assert_eq!(
        r.pixel(Pos2::new(20.0, outline.rect.center().y)),
        Some(t.color(Color::BorderHi)),
        "1 px BORDER_HI border"
    );
    let sec = r.marker("secondaryBtn").unwrap();
    assert_eq!(
        r.pixel(Pos2::new(20.0, sec.rect.center().y)),
        Some(t.color(Color::Border))
    );
    assert_eq!(
        r.pixel(Pos2::new(28.0, sec.rect.center().y)),
        Some(t.color(Color::Surface3))
    );
    let pill = r.marker("pillBtn").unwrap();
    assert!(
        (pill.rect.height() - 35.0).abs() <= 1.0,
        "pill: 21 + 7×2 = 35: {}",
        pill.rect.height()
    );
    assert_eq!(
        r.pixel(Pos2::new(28.0, pill.rect.center().y)),
        Some(t.color(Color::AccentSoft))
    );
    let ghost = r.marker("ghostBtn").unwrap();
    assert!(
        (ghost.rect.height() - 33.0).abs() <= 1.0,
        "ghost: 21 + 6×2 = 33: {}",
        ghost.rect.height()
    );
}

#[test]
fn toggle_geometry_and_states() {
    let t = tokens();
    let mut on = false;
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 100.0),
            )),
            |ui| {
                toggle::switch(ui, &t, &mut on, "toggle");
            },
        );
    });
    let m = r.marker("toggle").unwrap();
    assert_eq!(m.rect.size(), Vec2::new(38.0, 20.0));
    assert_eq!(
        r.pixel(Pos2::new(20.0 + 11.0, 30.0)),
        Some(t.color(Color::TextDim)),
        "off knob at x 5..17"
    );
    assert_eq!(
        r.pixel(Pos2::new(20.0 + 28.0, 30.0)),
        Some(t.color(Color::Surface1)),
        "off track"
    );
    assert_eq!(
        r.pixel(Pos2::new(39.0, 22.0)),
        Some(t.color(Color::BorderHi)),
        "off border (top edge)"
    );
    let mut on = true;
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 100.0),
            )),
            |ui| {
                toggle::switch(ui, &t, &mut on, "toggle");
            },
        );
    });
    assert_eq!(
        r.pixel(Pos2::new(20.0 + 27.0, 30.0)),
        Some(t.color(Color::Accent)),
        "on knob at x 21..33"
    );
    assert_eq!(
        r.pixel(Pos2::new(20.0 + 9.0, 30.0)),
        Some(t.color(Color::AccentSoft)),
        "on track"
    );
    assert_eq!(
        r.pixel(Pos2::new(39.0, 22.0)),
        Some(t.color(Color::Accent)),
        "on border (top edge)"
    );
    // Click toggles.
    let mut on = false;
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 100.0),
            )),
            |ui| {
                toggle::switch(ui, &t, &mut on, "toggle");
            },
        );
    });
    h.drag_at(Pos2::new(40.0, 30.0));
    h.step();
    h.drop_at(Pos2::new(40.0, 30.0));
    h.run_steps(2);
    drop(h);
    assert!(on);
}

#[test]
fn checkbox_and_radio_states() {
    let t = tokens();
    let mut on = true;
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 100.0),
            )),
            |ui| {
                check::checkbox(ui, &t, &mut on, "Show Ko-fi donation widget", "kofiCheck");
                check::radio(ui, &t, false, "Option", "radio");
                let mut off = false;
                check::checkbox(ui, &t, &mut off, "Unchecked", "offCheck");
            },
        );
    });
    let m = r.marker("kofiCheck").unwrap();
    assert!(
        (m.rect.height() - 26.0).abs() <= 1.0,
        "Qt checkbox row is 26 tall: {}",
        m.rect.height()
    );
    assert_eq!(m.text.as_ref().unwrap().px, 17.0);
    let box_c = Pos2::new(20.0 + 7.0, m.rect.center().y);
    assert_eq!(r.pixel(box_c), Some(t.color(Color::Accent)), "checked fill");
    let rr = r.marker("radio").unwrap();
    assert_eq!(
        r.pixel(Pos2::new(27.0, rr.rect.center().y)),
        Some(t.color(Color::Bg)),
        "unchecked radio fill"
    );
    let off = r.marker("offCheck").unwrap();
    assert_eq!(
        r.pixel(Pos2::new(20.0, off.rect.center().y)),
        Some(t.color(Color::Text)),
        "unchecked box border"
    );
    assert_eq!(
        r.pixel(Pos2::new(27.0, off.rect.center().y)),
        Some(t.color(Color::Bg)),
        "unchecked box fill"
    );
    let _ = rr;
}

#[test]
fn line_edit_border_states() {
    let t = tokens();
    let mut text = String::from("hello");
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 100.0),
            )),
            |ui| {
                input::line_edit(ui, &t, &mut text, "edit");
            },
        );
    });
    let m = r.marker("edit").unwrap();
    assert!(
        (m.rect.height() - 36.0).abs() <= 1.0,
        "22 + 6×2 + 2: {}",
        m.rect.height()
    );
    assert_eq!(
        r.pixel(Pos2::new(20.0, m.rect.center().y)),
        Some(t.color(Color::Border))
    );
    assert_eq!(
        r.pixel(Pos2::new(25.0, m.rect.center().y)),
        Some(t.color(Color::Surface2))
    );
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 100.0),
            )),
            |ui| {
                input::line_edit(ui, &t, &mut text, "edit");
            },
        );
    });
    h.hover_at(Pos2::new(100.0, 38.0));
    h.run_steps(2);
    let img = h.render().unwrap();
    let p = img.get_pixel(20, 38);
    assert_eq!(
        (p[0], p[1], p[2]),
        (0x49, 0x58, 0x4f),
        "hover border BORDER_HI"
    );
    h.drag_at(Pos2::new(100.0, 38.0));
    h.step();
    h.drop_at(Pos2::new(100.0, 38.0));
    h.run_steps(2);
    let img = h.render().unwrap();
    let p = img.get_pixel(20, 38);
    assert_eq!(
        (p[0], p[1], p[2]),
        (0x20, 0xe3, 0xa0),
        "focus border ACCENT"
    );
}

#[test]
fn number_spin_steps_and_clamps() {
    let t = tokens();
    let mut value: i64 = 199_900;
    let mut icons = IconCache::default();
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(190.0, 100.0),
            )),
            |ui| {
                input::number_spin(
                    ui,
                    &t,
                    &mut icons,
                    &mut value,
                    500..=200_000,
                    500,
                    " kbps",
                    "bitrate",
                );
            },
        );
    });
    h.step();
    let markers = fthrust_ui::fidelity::take(&h.ctx);
    let m = markers.iter().find(|m| m.name == "bitrate").unwrap();
    assert_eq!(m.rect.height(), 34.0);
    assert_eq!(m.rect.width(), 190.0);
    let up = markers
        .iter()
        .find(|m| m.name == "bitrate/fthrStepUp")
        .unwrap();
    assert_eq!(up.rect.size(), Vec2::new(28.0, 32.0));
    assert_eq!(up.rect.min, Pos2::new(20.0 + 190.0 - 29.0, 21.0));
    let down = markers
        .iter()
        .find(|m| m.name == "bitrate/fthrStepDown")
        .unwrap();
    assert_eq!(down.rect.min.x, 20.0 + 190.0 - 57.0);
    let up_c = up.rect.center();
    let down_c = down.rect.center();
    for _ in 0..2 {
        h.drag_at(up_c);
        h.step();
        h.drop_at(up_c);
        h.run_steps(2);
    }
    for _ in 0..3 {
        h.drag_at(down_c);
        h.step();
        h.drop_at(down_c);
        h.run_steps(2);
    }
    drop(h);
    assert_eq!(value, 198_500, "199900 → 200000 (clamped) → 198500");
}

#[test]
fn slider_click_jumps_and_handle_colors() {
    let t = tokens();
    let mut value = 50.0f32;
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(212.0, 100.0),
            )),
            |ui| {
                slider::settings_slider(ui, &t, &mut value, 0.0..=100.0, "slider");
            },
        );
    });
    let m = r.marker("slider").unwrap();
    assert_eq!(m.rect.height(), 26.0);
    let cy = m.rect.center().y;
    assert_eq!(
        r.pixel(Pos2::new(40.0, cy)),
        Some(t.color(Color::Accent)),
        "sub-page ACCENT"
    );
    assert_eq!(
        r.pixel(Pos2::new(200.0, cy)),
        Some(t.color(Color::Border)),
        "add-page BORDER"
    );
    assert_eq!(
        r.pixel(Pos2::new(20.0 + 106.0, cy)),
        Some(t.color(Color::Accent)),
        "handle at 50%"
    );
    let mut value = 0.0f32;
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(212.0, 100.0),
            )),
            |ui| {
                slider::settings_slider(ui, &t, &mut value, 0.0..=100.0, "slider");
            },
        );
    });
    h.drag_at(Pos2::new(20.0 + 6.0 + 150.0, 33.0));
    h.step();
    h.drop_at(Pos2::new(20.0 + 6.0 + 150.0, 33.0));
    h.run_steps(2);
    drop(h);
    assert!(
        (value - 75.0).abs() < 1.0,
        "click at 75% of the 200 px span: {value}"
    );
}

#[test]
fn top_bar_button_arrow_and_states() {
    let t = tokens();
    let mut icons = IconCache::default();
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(14.0, 6.0),
                Vec2::new(300.0, 60.0),
            )),
            |ui| {
                buttons::top_bar_button(ui, &t, &mut icons, "Source", false, 120.0, "source_btn");
            },
        );
    });
    let m = r.marker("source_btn").unwrap();
    assert_eq!(m.rect.size(), Vec2::new(120.0, 44.0));
    assert_eq!(
        r.pixel(Pos2::new(14.0 + 4.0, 28.0)),
        Some(t.color(Color::Surface3))
    );
    assert_eq!(m.icon.as_ref().unwrap().0, "icons/dropdown.svg");
    assert_eq!(m.text.as_ref().unwrap().px, 15.0);
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(14.0, 6.0),
                Vec2::new(300.0, 60.0),
            )),
            |ui| {
                buttons::top_bar_button(ui, &t, &mut icons, "Source", true, 120.0, "source_btn");
            },
        );
    });
    h.hover_at(Pos2::new(60.0, 28.0));
    h.run_steps(3);
    let img = h.render().unwrap();
    let p = img.get_pixel(18, 28);
    assert_eq!((p[0], p[1], p[2]), (0x10, 0x15, 0x12), "hover bg SURFACE_2");
}

#[test]
fn section_header_and_separator() {
    let t = tokens();
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 100.0),
            )),
            |ui| {
                section::flat_header(ui, &t, "System", "System");
                ui.add_space(10.0);
                section::hsep(ui, &t, "hsep");
            },
        );
    });
    let h = r.marker("QLabel[System]").unwrap();
    assert!(
        (h.rect.height() - 30.0).abs() <= 1.0,
        "Qt H3 20 px label is 30 tall: {}",
        h.rect.height()
    );
    assert!(
        (h.rect.width() - 58.0).abs() <= 1.5,
        "Qt 'System' advance 58: {}",
        h.rect.width()
    );
    assert_eq!(h.text.as_ref().unwrap().weight, Weight::W600);
    let rule_y = h.rect.center().y;
    assert_eq!(
        r.pixel(Pos2::new(200.0, rule_y.floor())),
        Some(t.color(Color::ShellDivider)),
        "rule right of the title"
    );
    let sep = r.marker("hsep").unwrap();
    assert_eq!(sep.rect.height(), 1.0);
    assert_eq!(
        r.pixel(Pos2::new(100.0, sep.rect.min.y)),
        Some(t.color(Color::ShellDivider))
    );
}

#[test]
fn hover_transition_takes_120ms_unless_reduced() {
    let mut t = Tokens::default_palette();
    t.reduce_motion = false;
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::new(20.0, 20.0),
                Vec2::new(300.0, 100.0),
            )),
            |ui| {
                buttons::primary(ui, &t, "New", "newBtn");
            },
        );
    });
    h.hover_at(Pos2::new(40.0, 40.0));
    h.step(); // hover registered, blend starts
    h.step();
    let img = h.render().unwrap();
    let p = img.get_pixel(28, 40);
    assert_ne!(
        (p[0], p[1], p[2]),
        (0xf3, 0xf6, 0xf4),
        "mid-transition after one frame"
    );
    assert_ne!((p[0], p[1], p[2]), (0x20, 0xe3, 0xa0));
    h.run_steps(30);
    let img = h.render().unwrap();
    let p = img.get_pixel(28, 40);
    assert_eq!((p[0], p[1], p[2]), (0xf3, 0xf6, 0xf4), "settled on TEXT");
    assert_eq!(Icon::Dropdown.asset(), "icons/dropdown.svg");
}
