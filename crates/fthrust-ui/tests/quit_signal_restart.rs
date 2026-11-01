//! The process-wide "quitting" flag, in a test binary of its own (setting it
//! changes what every supervisor in the process does).

use fthrust_core::capture_config::CaptureConfig;
use fthrust_ipc::EngineConfig;
use fthrust_ui::services::engine::{AUTO_RESTART_DELAY_S, Effect, Input, Supervisor};

/// Final review of the audit fixes: a logout signals the engine and the app
/// together. Once the quit signal has arrived, an engine that exits is not
/// restarted, even before the shutdown sends `Quitting`.
#[test]
fn an_engine_that_exits_during_a_quit_is_not_restarted() {
    let d = tempfile::tempdir().unwrap();
    let s = fthrust_ui::state::AppState::for_tests(d.path(), serde_json::json!({}));
    let config = EngineConfig {
        capture: CaptureConfig::from_settings(&s.settings).unwrap(),
        max_buffer_mb: 256,
        backend_override: None,
        automatic_start: false,
    };
    let mut sv = Supervisor::new();
    let pid = 4242;
    sv.handle(Input::Start(config), 0.0);
    sv.handle(Input::Spawned { pid }, 0.0);
    sv.handle(
        Input::Hello {
            pid,
            version: "x".into(),
        },
        0.0,
    );
    sv.handle(Input::Configured { warnings: vec![] }, 0.0);
    // The engine dies first, and its restart is armed; then the signal.
    sv.handle(Input::ProcessExited { code: Some(0) }, 1.0);
    assert!(sv.next_deadline().is_some(), "the restart is armed");
    fthrust_ui::services::set_quitting();
    let fx = sv.handle(Input::Tick, 1.0 + AUTO_RESTART_DELAY_S + 0.5);
    assert!(!fx.iter().any(|e| matches!(e, Effect::Spawn)), "{fx:?}");
    assert_eq!(sv.next_deadline(), None);
}
