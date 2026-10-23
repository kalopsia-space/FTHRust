//! The app's egui context: egui's built-in Ctrl+Q quit and keyboard zoom
//! are off (Ctrl+Q is the editor's quick crop; the reference has neither).

use egui::{Event, Key, Modifiers, RawInput, ViewportCommand, ViewportId};

fn press(key: Key, modifiers: Modifiers) -> RawInput {
    RawInput {
        events: vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }],
        ..Default::default()
    }
}

#[test]
fn ctrl_q_does_not_close_the_app() {
    let ctx = fthrust_ui::runner::new_context();
    let mut out = ctx.run_pass(press(Key::Q, Modifiers::CTRL | Modifiers::COMMAND), |_| {});
    out.textures_delta.clear();
    let closes = out
        .viewport_output
        .get(&ViewportId::ROOT)
        .is_some_and(|v| v.commands.contains(&ViewportCommand::Close));
    assert!(!closes, "Ctrl+Q closed the window");
}

#[test]
fn ctrl_plus_does_not_zoom() {
    let ctx = fthrust_ui::runner::new_context();
    for key in [Key::Plus, Key::Equals] {
        let mut out = ctx.run_pass(press(key, Modifiers::CTRL | Modifiers::COMMAND), |_| {});
        out.textures_delta.clear();
    }
    assert_eq!(ctx.zoom_factor(), 1.0);
}
