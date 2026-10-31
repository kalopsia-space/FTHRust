//! The process-wide marker switch, in its own test binary: flipping it
//! while other tests record markers would race them.

/// Audit 2026-10-31: with recording off (the app without the bench's
/// dump), markers are not built or registered.
#[test]
fn markers_can_be_switched_off() {
    let ctx = egui::Context::default();
    let mut count = 0;
    for on in [false, true] {
        fthrust_ui::fidelity::set_recording(on);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            fthrust_ui::fidelity::mark(ui, "probe", egui::Rect::ZERO)
                .text("x", 10.0, fthrust_ui::fonts::Weight::W400)
                .done();
        });
        out.textures_delta.clear();
        let taken = fthrust_ui::fidelity::take(&ctx);
        if on {
            count = taken.len();
        } else {
            assert!(taken.is_empty(), "nothing recorded while off");
        }
    }
    assert_eq!(count, 1);
}
