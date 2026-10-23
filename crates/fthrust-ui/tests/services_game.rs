//! Plan 5 Task 14: game detection and focus pause from the compositor's
//! toplevel list.

use fthrust_core::game_rules::{CustomGameRule, normalise_rules};
use fthrust_ui::services::focus::{FocusChange, FocusMonitor};
use fthrust_ui::services::game::{candidates, running_windows};
use fthrust_ui::services::status::FOCUS_PAUSED;
use fthrust_ui::services::{CardKind, Event, Request, Services};
use fthrust_ui::state::AppState;
use fthrust_wayland::toplevels::Toplevel;
use serde_json::json;

fn state(settings: serde_json::Value) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), settings);
    s.tokens.reduce_motion = true;
    (tmp, s)
}

fn tl(id: u32, title: &str, app_id: &str, fullscreen: bool, activated: bool) -> Toplevel {
    Toplevel {
        id,
        title: title.into(),
        app_id: app_id.into(),
        fullscreen,
        activated,
        minimized: false,
    }
}

fn rules(v: serde_json::Value) -> Vec<CustomGameRule> {
    normalise_rules(&v)
}

#[test]
fn fullscreen_toplevel_is_candidate_rule_and_blacklist_applied() {
    let snap = vec![
        tl(1, "Celeste", "celeste", true, true),
        tl(2, "Firefox", "firefox", false, false),
        tl(3, "Hollow Knight", "hollow_knight", false, false),
        tl(4, "mpv", "mpv", true, false),
        tl(5, "", "steam_app_1", true, false),
    ];
    let custom = rules(json!([{"title_contains": "hollow", "app_id": ""}]));
    let blacklist = rules(json!([{"title_contains": "", "app_id": "mpv"}]));
    let c = candidates(&snap, &custom, &blacklist);
    let ids: Vec<u32> = c.iter().map(|c| c.id).collect();
    assert_eq!(
        ids,
        vec![1, 3],
        "fullscreen + custom rule; blacklisted mpv and untitled excluded"
    );
    assert_eq!(c[1].title, "Hollow Knight");
    let windows = running_windows(&snap);
    assert_eq!(
        windows,
        vec![
            "Celeste · celeste",
            "Firefox · firefox",
            "Hollow Knight · hollow_knight",
            "mpv · mpv"
        ]
    );
}

#[test]
fn prompt_mode_shows_card_and_confirm_switches() {
    let (_tmp, mut s) =
        state(json!({"game_detection_enabled": true, "game_detection_mode": "prompt"}));
    s.hotkeys.set_keyboard("confirm_game_detection", "F8");
    s.hotkeys.set_keyboard("dismiss_game_detection", "F7");
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.inject(Event::Toplevels(vec![
        tl(7, "Celeste", "celeste", true, true),
        tl(2, "Firefox", "firefox", false, false),
    ]));
    services.tick(&mut s);
    assert_eq!(
        services.cards_for_tests(),
        vec![CardKind::Prompt {
            text: "Celeste detected — [F8] Record  [F7] Dismiss".into()
        }]
    );
    assert_eq!(
        s.running_windows,
        vec!["Celeste · celeste", "Firefox · firefox"]
    );
    assert_eq!(
        s.settings.get_str("capture_mode"),
        Some("desktop"),
        "prompt mode does not switch yet"
    );
    s.game_confirm_requested = true;
    services.tick(&mut s);
    assert_eq!(s.settings.get_str("capture_mode"), Some("window"));
    assert_eq!(s.settings.get_str("target_window_name"), Some("Celeste"));
    assert_eq!(s.settings.get_str("target_app_id"), Some("celeste"));
    assert_eq!(s.source_label, "CELESTE");
    assert_eq!(
        services.cards_for_tests().last(),
        Some(&CardKind::Capturing {
            source: "Celeste".into()
        })
    );
    assert!(
        services
            .drain_requests_for_tests()
            .contains(&Request::RestartEngine)
    );
    // The same game again does not re-prompt.
    services.inject(Event::Toplevels(vec![tl(
        7, "Celeste", "celeste", true, true,
    )]));
    services.tick(&mut s);
    assert_eq!(services.cards_for_tests().len(), 2);
}

#[test]
fn prompt_dismiss_and_timeout_suppress_candidate() {
    let (_tmp, mut s) =
        state(json!({"game_detection_enabled": true, "game_detection_mode": "prompt"}));
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.inject(Event::Toplevels(vec![tl(
        7, "Celeste", "celeste", true, true,
    )]));
    services.tick(&mut s);
    assert_eq!(services.cards_for_tests().len(), 1);
    s.game_dismiss_requested = true;
    services.tick(&mut s);
    services.inject(Event::Toplevels(vec![tl(
        7, "Celeste", "celeste", true, true,
    )]));
    services.tick(&mut s);
    assert_eq!(
        services.cards_for_tests().len(),
        1,
        "dismissed candidates are suppressed"
    );
    assert_eq!(s.settings.get_str("capture_mode"), Some("desktop"));
    // A new candidate prompts; after 15 s without an answer it is suppressed.
    services.inject(Event::Toplevels(vec![
        tl(7, "Celeste", "celeste", true, true),
        tl(8, "Hades", "hades", true, true),
    ]));
    services.tick(&mut s);
    assert_eq!(services.cards_for_tests().len(), 2);
    services.advance_clock_for_tests(15.5);
    services.tick(&mut s);
    s.game_confirm_requested = true;
    services.tick(&mut s);
    assert_eq!(
        s.settings.get_str("capture_mode"),
        Some("desktop"),
        "confirm after the timeout does nothing"
    );
}

#[test]
fn auto_mode_switches_and_restarts_engine() {
    let (_tmp, mut s) =
        state(json!({"game_detection_enabled": true, "game_detection_mode": "auto"}));
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.inject(Event::Toplevels(vec![tl(
        7, "Hades II", "hades2", true, true,
    )]));
    services.tick(&mut s);
    assert_eq!(s.settings.get_str("capture_mode"), Some("window"));
    assert_eq!(s.settings.get_str("target_window_name"), Some("Hades II"));
    assert_eq!(s.source_label, "HADES II");
    assert_eq!(
        services.cards_for_tests(),
        vec![CardKind::Capturing {
            source: "Hades II".into()
        }]
    );
    assert!(
        services
            .drain_requests_for_tests()
            .contains(&Request::RestartEngine)
    );
    // Disabled: nothing happens.
    let (_tmp2, mut s2) =
        state(json!({"game_detection_enabled": false, "game_detection_mode": "auto"}));
    let mut services2 = Services::start_for_tests(&ctx);
    services2.inject(Event::Toplevels(vec![tl(
        7, "Hades II", "hades2", true, true,
    )]));
    services2.tick(&mut s2);
    assert_eq!(s2.settings.get_str("capture_mode"), Some("desktop"));
    assert!(services2.cards_for_tests().is_empty());
}

#[test]
fn game_closed_falls_back_to_desktop_when_enabled() {
    let (_tmp, mut s) = state(
        json!({"game_detection_enabled": true, "game_detection_mode": "auto", "game_detection_fallback_desktop": true}),
    );
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.inject(Event::Toplevels(vec![tl(
        7, "Hades II", "hades2", true, true,
    )]));
    services.tick(&mut s);
    services.drain_requests_for_tests();
    services.inject(Event::Toplevels(vec![tl(
        2, "Firefox", "firefox", false, true,
    )]));
    services.tick(&mut s);
    assert_eq!(s.settings.get_str("capture_mode"), Some("desktop"));
    assert_eq!(s.source_label, "DESKTOP");
    assert_eq!(
        services.cards_for_tests().last(),
        Some(&CardKind::Prompt {
            text: "Game closed — switched back to Desktop".into()
        })
    );
    assert!(
        services
            .drain_requests_for_tests()
            .contains(&Request::RestartEngine)
    );
    // Without the fallback the window source stays.
    let (_tmp2, mut s2) = state(
        json!({"game_detection_enabled": true, "game_detection_mode": "auto", "game_detection_fallback_desktop": false}),
    );
    let mut services2 = Services::start_for_tests(&ctx);
    services2.inject(Event::Toplevels(vec![tl(
        7, "Hades II", "hades2", true, true,
    )]));
    services2.tick(&mut s2);
    services2.inject(Event::Toplevels(vec![]));
    services2.tick(&mut s2);
    assert_eq!(s2.settings.get_str("capture_mode"), Some("window"));
    assert_eq!(s2.source_label, "HADES II");
}

#[test]
fn focus_lost_pauses_and_regained_resumes() {
    let mut m = FocusMonitor::default();
    m.set_target(Some("Hades II"));
    let game = tl(7, "Hades II - Early Access", "hades2", true, true);
    let other = tl(2, "Firefox", "firefox", false, true);
    let mut game_inactive = game.clone();
    game_inactive.activated = false;
    assert_eq!(
        m.observe(&[game.clone(), tl(2, "Firefox", "firefox", false, false)]),
        None,
        "focused from the start: no change"
    );
    assert_eq!(
        m.observe(&[game_inactive.clone(), other.clone()]),
        Some(FocusChange::Lost)
    );
    assert_eq!(m.observe(&[game_inactive.clone(), other.clone()]), None);
    assert_eq!(
        m.observe(std::slice::from_ref(&game)),
        Some(FocusChange::Regained)
    );
    m.set_target(None);
    assert_eq!(
        m.observe(std::slice::from_ref(&other)),
        None,
        "no target: never pauses"
    );

    let (_tmp, mut s) = state(
        json!({"anticheat_detection_enabled": true, "capture_mode": "window", "target_window_name": "Hades II"}),
    );
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.inject(Event::EngineConnected(true));
    services.inject(Event::Status {
        text: "CAPTURING".into(),
        style: fthrust_ui::state::StatusStyle::Active,
    });
    services.tick(&mut s);
    services.inject(Event::Toplevels(vec![game.clone()]));
    services.tick(&mut s);
    services.inject(Event::Toplevels(vec![game_inactive.clone(), other.clone()]));
    services.tick(&mut s);
    assert!(
        services
            .drain_requests_for_tests()
            .contains(&Request::PauseCapture)
    );
    assert_eq!(s.status.text, FOCUS_PAUSED);
    services.inject(Event::Toplevels(vec![game.clone()]));
    services.tick(&mut s);
    assert!(
        services
            .drain_requests_for_tests()
            .contains(&Request::ResumeCapture)
    );
    assert_eq!(s.status.text, "STARTING CAPTURE");
    // Disabled: no pause.
    let (_tmp2, mut s2) = state(
        json!({"anticheat_detection_enabled": false, "capture_mode": "window", "target_window_name": "Hades II"}),
    );
    let mut services2 = Services::start_for_tests(&ctx);
    services2.inject(Event::Toplevels(vec![game.clone()]));
    services2.tick(&mut s2);
    services2.inject(Event::Toplevels(vec![game_inactive, other]));
    services2.tick(&mut s2);
    assert!(
        !services2
            .drain_requests_for_tests()
            .contains(&Request::PauseCapture)
    );
}

#[test]
fn unavailable_protocol_sets_label_and_warning() {
    let (_tmp, mut s) = state(json!({"game_detection_enabled": true}));
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    assert_eq!(
        fthrust_ui::shell::toolbar::game_summary(&s.settings, true),
        "Game detection: On"
    );
    services.inject(Event::GameDetectionAvailability(false));
    services.tick(&mut s);
    assert!(!s.game_detection_available);
    assert_eq!(
        fthrust_ui::shell::toolbar::game_summary(&s.settings, s.game_detection_available),
        "Game detection unavailable"
    );
    let e = s.error_bar.current().unwrap();
    assert_eq!(e.title, "LIMITED FEATURE SUPPORT");
    assert_eq!(
        e.detail,
        "Game detection and focus pause need a compositor that exposes its window list (wlr-foreign-toplevel-management). Hotkeys and capture are unaffected."
    );
    assert_eq!(s.running_windows, Vec::<String>::new());
}
