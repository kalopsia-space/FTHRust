mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::fonts::Weight;
use fthrust_ui::icons::IconCache;
use fthrust_ui::tokens::{Color, Tokens};
use fthrust_ui::widgets::{accordion, combo, dialog, popup, scroll};

const CANVAS: Vec2 = Vec2::new(500.0, 400.0);

fn tokens() -> Tokens {
    let mut t = Tokens::default_palette();
    t.reduce_motion = true;
    t.wayland = true;
    t
}

fn at(x: f32, y: f32, w: f32, h: f32) -> egui::UiBuilder {
    egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h)))
}

#[test]
fn combo_closed_geometry_and_popup_selection() {
    let t = tokens();
    let mut icons = IconCache::default();
    let mut state = combo::Combo::new(
        vec![
            "All notifications".into(),
            "Errors only".into(),
            "None".into(),
        ],
        0,
    );
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(at(20.0, 20.0, 300.0, 100.0), |ui| {
            combo::Combo::ui(&mut state, ui, &t, &mut icons, "notifCombo");
        });
    });
    let m = r.marker("notifCombo").unwrap();
    assert_eq!(
        m.rect.size(),
        Vec2::new(300.0, 38.0),
        "Qt QComboBox is 38 tall and expands"
    );
    assert_eq!(r.pixel(Pos2::new(20.0, 39.0)), Some(t.color(Color::Border)));
    assert_eq!(
        r.pixel(Pos2::new(25.0, 39.0)),
        Some(t.color(Color::Surface3))
    );
    assert_eq!(m.text.as_ref().unwrap().content, "All notifications");
    assert_eq!(m.text.as_ref().unwrap().px, 15.0);
    assert_eq!(
        m.icon.as_ref().unwrap(),
        &("icons/dropdown.svg".to_owned(), 14.0)
    );
    // Open with a click, choose the third item.
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(at(20.0, 20.0, 300.0, 100.0), |ui| {
            combo::Combo::ui(&mut state, ui, &t, &mut icons, "notifCombo");
        });
    });
    h.drag_at(Pos2::new(100.0, 39.0));
    h.step();
    h.drop_at(Pos2::new(100.0, 39.0));
    h.run_steps(3);
    let markers = fthrust_ui::fidelity::take(&h.ctx);
    h.step();
    let markers = if markers.is_empty() {
        fthrust_ui::fidelity::take(&h.ctx)
    } else {
        markers
    };
    let list = markers
        .iter()
        .find(|m| m.name == "notifCombo/list")
        .expect("popup list marker");
    assert_eq!(
        list.rect.min,
        Pos2::new(20.0, 58.0),
        "list sits under the combo"
    );
    assert_eq!(list.rect.width(), 300.0);
    let item = markers
        .iter()
        .find(|m| m.name == "notifCombo/item[None]")
        .unwrap();
    assert_eq!(item.rect.height(), 26.0);
    let img = h.render().unwrap();
    let p = img.get_pixel(30, 71);
    assert_eq!(
        (p[0], p[1], p[2]),
        (0x14, 0x1b, 0x17),
        "list background SURFACE_3"
    );
    let c = item.rect.center();
    h.drag_at(c);
    h.step();
    h.drop_at(c);
    h.run_steps(3);
    drop(h);
    assert_eq!(state.selected, 2);
}

#[test]
fn accordion_header_and_body_toggle() {
    let t = tokens();
    let mut icons = IconCache::default();
    let mut open = false;
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(at(20.0, 20.0, 400.0, 300.0), |ui| {
            accordion::section(
                ui,
                &t,
                &mut icons,
                "01",
                "Typography",
                &mut open,
                "typographySection",
                |ui| {
                    ui.allocate_exact_size(Vec2::new(100.0, 50.0), egui::Sense::hover());
                },
            );
        });
    });
    let m = r.marker("typographySection").unwrap();
    assert_eq!(
        m.rect.size(),
        Vec2::new(400.0, 44.0),
        "collapsed: header only"
    );
    assert_eq!(
        r.pixel(Pos2::new(20.0, 42.0)),
        Some(t.color(Color::Border)),
        "1 px BORDER"
    );
    assert_eq!(
        r.pixel(Pos2::new(200.0, 42.0)),
        Some(t.color(Color::Surface3))
    );
    let num = r.marker("typographySection/number").unwrap();
    assert_eq!(num.rect.min.x, 36.0, "16 px header margin");
    assert_eq!(num.text.as_ref().unwrap().px, 17.0);
    assert_eq!(num.text.as_ref().unwrap().weight, Weight::W600);
    let mut open = true;
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(at(20.0, 20.0, 400.0, 300.0), |ui| {
            accordion::section(
                ui,
                &t,
                &mut icons,
                "01",
                "Typography",
                &mut open,
                "typographySection",
                |ui| {
                    ui.allocate_exact_size(Vec2::new(100.0, 50.0), egui::Sense::hover());
                },
            );
        });
    });
    let m = r.marker("typographySection").unwrap();
    assert_eq!(
        m.rect.height(),
        44.0 + 16.0 + 50.0 + 16.0,
        "expanded: body margins 16"
    );
    let mut open = false;
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(at(20.0, 20.0, 400.0, 300.0), |ui| {
            accordion::section(
                ui,
                &t,
                &mut icons,
                "01",
                "Typography",
                &mut open,
                "typographySection",
                |_| {},
            );
        });
    });
    h.drag_at(Pos2::new(200.0, 42.0));
    h.step();
    h.drop_at(Pos2::new(200.0, 42.0));
    h.run_steps(2);
    drop(h);
    assert!(open);
}

#[test]
fn popup_panel_places_under_anchor_and_clamps_to_screen() {
    let t = tokens();
    let mut state = popup::PanelState::default();
    state.open = true;
    let anchor = Rect::from_min_size(Pos2::new(380.0, 10.0), Vec2::new(100.0, 44.0));
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        popup::Panel::new("src_popup", anchor, 300.0).show(ui, &t, &mut state, |ui| {
            ui.allocate_exact_size(Vec2::new(200.0, 60.0), egui::Sense::hover());
        });
    });
    let m = r.marker("src_popup").unwrap();
    assert_eq!(
        m.rect.max.x, 480.0,
        "right-aligned with the anchor when it would overflow"
    );
    let wide = Rect::from_min_size(Pos2::new(450.0, 10.0), Vec2::new(100.0, 44.0));
    let mut state2 = popup::PanelState::default();
    state2.open = true;
    let r2 = common::render(CANVAS, 1.0, &t, |ui| {
        popup::Panel::new("src_popup", wide, 300.0).show(ui, &t, &mut state2, |ui| {
            ui.allocate_exact_size(Vec2::new(200.0, 60.0), egui::Sense::hover());
        });
    });
    assert_eq!(
        r2.marker("src_popup").unwrap().rect.max.x,
        500.0 - 8.0,
        "clamped 8 px from the right edge"
    );
    assert_eq!(m.rect.min.y, 54.0, "directly under the anchor");
    assert_eq!(m.rect.width(), 300.0, "min width 300");
    assert_eq!(
        r.pixel(Pos2::new(m.rect.min.x, m.rect.center().y)),
        Some(t.color(Color::BorderHi))
    );
    assert_eq!(
        r.pixel(Pos2::new(m.rect.min.x + 5.0, m.rect.center().y)),
        Some(t.color(Color::Surface2))
    );
    // Outside click closes.
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        popup::Panel::new("src_popup", anchor, 300.0).show(ui, &t, &mut state, |ui| {
            ui.allocate_exact_size(Vec2::new(200.0, 60.0), egui::Sense::hover());
        });
    });
    h.drag_at(Pos2::new(50.0, 300.0));
    h.step();
    h.drop_at(Pos2::new(50.0, 300.0));
    h.run_steps(2);
    drop(h);
    assert!(!state.open);
}

#[test]
fn popup_fade_runs_only_off_wayland() {
    let mut t = Tokens::default_palette();
    t.reduce_motion = false;
    t.wayland = false;
    let mut state = popup::PanelState::default();
    state.open = true;
    let anchor = Rect::from_min_size(Pos2::new(20.0, 10.0), Vec2::new(100.0, 44.0));
    let body = |ui: &mut egui::Ui| {
        ui.allocate_exact_size(Vec2::new(200.0, 60.0), egui::Sense::hover());
    };
    // 1 ms frames: the harness's own settle loop stays well inside the 180 ms fade.
    let mut h = common::harness_with_dt(CANVAS, 1.0, &t, 0.001, |ui| {
        popup::Panel::new("p", anchor, 300.0).show(ui, &t, &mut state, body);
    });
    h.run_steps(10);
    let img = h.render().unwrap();
    let p = img.get_pixel(25, 80);
    assert_ne!((p[0], p[1], p[2]), (0x10, 0x15, 0x12), "mid-fade on X11");
    assert_ne!((p[0], p[1], p[2]), (0, 0, 0), "but already visible");
    h.run_steps(300);
    let img = h.render().unwrap();
    let p = img.get_pixel(25, 80);
    assert_eq!((p[0], p[1], p[2]), (0x10, 0x15, 0x12), "faded in");
    drop(h);
    t.wayland = true;
    let mut state = popup::PanelState::default();
    state.open = true;
    let mut h = common::harness(CANVAS, 1.0, &t, |ui| {
        popup::Panel::new("p", anchor, 300.0).show(ui, &t, &mut state, body);
    });
    h.step();
    let img = h.render().unwrap();
    let p = img.get_pixel(25, 80);
    assert_eq!((p[0], p[1], p[2]), (0x10, 0x15, 0x12), "no fade on Wayland");
}

#[test]
fn dialog_frame_matches_qt_message_layout() {
    let t = tokens();
    let mut icons = IconCache::default();
    let r = common::render(Vec2::new(340.0, 177.0), 1.0, &t, |ui| {
        dialog::Frame::new("Clip not found", 340.0).show_at(
            ui,
            &t,
            &mut icons,
            Pos2::ZERO,
            |ui| {
                dialog::message_body(ui, &t, "The clip is no longer on disk.", 308.0);
            },
            |ui| {
                fthrust_ui::widgets::buttons::primary(ui, &t, "OK", "QPushButton[OK]");
            },
        );
    });
    let header = r.marker("fthrDialogHeader").unwrap();
    assert_eq!(
        header.rect,
        Rect::from_min_size(Pos2::ZERO, Vec2::new(340.0, 38.0))
    );
    assert_eq!(
        r.pixel(Pos2::new(170.0, 20.0)),
        Some(t.color(Color::Surface2))
    );
    let title = r.marker("fthrDialogTitle").unwrap();
    assert_eq!(title.rect.min.x, 12.0);
    assert_eq!(title.text.as_ref().unwrap().px, 14.0);
    let close = r.marker("fthrDialogClose").unwrap();
    assert_eq!(
        close.rect,
        Rect::from_min_size(Pos2::new(304.0, 3.0), Vec2::new(30.0, 30.0))
    );
    let ok = r.marker("QPushButton[OK]").unwrap();
    assert!((ok.rect.min.x - 272.0).abs() <= 1.5, "{:?}", ok.rect);
    assert_eq!(ok.rect.min.y, 122.0);
    let frame = r.marker("fthrDialog").unwrap();
    assert_eq!(frame.rect.height(), 177.0, "38 + 16 + 52 + 8 + 8 + 41 + 14");
    assert_eq!(
        r.pixel(Pos2::new(0.0, 100.0)),
        Some(t.color(Color::BorderHi)),
        "1 px BORDER_HI"
    );
    assert_eq!(r.pixel(Pos2::new(5.0, 100.0)), Some(t.color(Color::Bg)));
    common::compare(&r, "dialog-message", &t, &h_ctx(&r));
}

fn h_ctx(_r: &common::Rendered) -> egui::Context {
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, &tokens());
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    ctx
}

#[test]
fn scroll_area_uses_8px_bars_with_token_colours() {
    let t = tokens();
    let r = common::render(CANVAS, 1.0, &t, |ui| {
        ui.scope_builder(at(20.0, 20.0, 200.0, 100.0), |ui| {
            scroll::vertical(ui, &t, "list").show(ui, |ui| {
                for i in 0..40 {
                    ui.allocate_exact_size(Vec2::new(150.0, 20.0), egui::Sense::hover());
                    let _ = i;
                }
            });
        });
    });
    // Handle in the right 8 px, BORDER_HI; track BG.
    let p = r.pixel(Pos2::new(20.0 + 200.0 - 4.0, 30.0)).unwrap();
    assert_eq!(
        p,
        t.color(Color::BorderHi),
        "handle at the top of the track"
    );
    let p = r.pixel(Pos2::new(20.0 + 200.0 - 4.0, 110.0)).unwrap();
    assert_eq!(p, t.color(Color::Bg), "track below the handle");
    assert_eq!(
        r.pixel(Pos2::new(20.0 + 200.0 - 9.0, 30.0)),
        Some(t.color(Color::Bg)),
        "content area left of the 8 px bar"
    );
}
